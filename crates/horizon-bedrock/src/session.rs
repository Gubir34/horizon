//! RakNet's connected layer: frame sets, acknowledgements, resends, ordering and split packets.
//!
//! A [`Session`] is a state machine with no socket and no clock of its own: datagrams go in through
//! [`Session::receive`], payloads come out, and [`Session::flush`] says which datagrams to send now. The
//! caller supplies the time, so the tests run in no real time at all.
//!
//! Format from the public description of RakNet: a datagram starts with a flag byte (`0x80` set; `0x40`
//! acknowledgement, `0x20` negative acknowledgement), a frame set then holds a 3-byte little-endian sequence
//! number and frames, each with its reliability, optional indexes and optional split header.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::time::{Duration, Instant};

use crate::raknet::DecodeError;

pub const FLAG_VALID: u8 = 0x80;
pub const FLAG_ACK: u8 = 0x40;
pub const FLAG_NACK: u8 = 0x20;
/// What a frame set's flag byte usually is: valid, plus "more may follow" the way senders set it.
pub const FLAGS_FRAME_SET: u8 = 0x84;
/// Bytes of a frame set before its frames: the flag byte and the sequence number.
const SET_HEADER: usize = 4;
/// The most a frame's header takes: flags, length, reliable, sequence and order indexes, channel, split header.
const FRAME_HEADER_MAX: usize = 1 + 2 + 3 + 3 + 3 + 1 + 10;
/// How long a datagram waits for its acknowledgement before it is sent again.
pub const RESEND_AFTER: Duration = Duration::from_millis(500);
/// An unacknowledged datagram this old (or sent this many times) means the peer is gone.
pub const GIVE_UP_AFTER: Duration = Duration::from_secs(10);
/// Ordered channels RakNet has.
pub const CHANNELS: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reliability {
    Unreliable,
    UnreliableSequenced,
    Reliable,
    ReliableOrdered,
    ReliableSequenced,
}
impl Reliability {
    fn id(self) -> u8 {
        match self {
            Self::Unreliable => 0,
            Self::UnreliableSequenced => 1,
            Self::Reliable => 2,
            Self::ReliableOrdered => 3,
            Self::ReliableSequenced => 4,
        }
    }
    fn from_id(id: u8) -> Option<Self> {
        Some(match id {
            0 | 5 => Self::Unreliable,
            1 => Self::UnreliableSequenced,
            2 | 6 => Self::Reliable,
            3 | 7 => Self::ReliableOrdered,
            4 => Self::ReliableSequenced,
            _ => return None,
        })
    }
    fn reliable(self) -> bool {
        matches!(
            self,
            Self::Reliable | Self::ReliableOrdered | Self::ReliableSequenced
        )
    }
    fn sequenced(self) -> bool {
        matches!(self, Self::UnreliableSequenced | Self::ReliableSequenced)
    }
    fn ordered(self) -> bool {
        matches!(
            self,
            Self::ReliableOrdered | Self::ReliableSequenced | Self::UnreliableSequenced
        )
    }
}

/// The header of a split packet's piece: all pieces of one packet share `id` and `count`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Split {
    pub count: u32,
    pub id: u16,
    pub index: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    pub reliability: Reliability,
    pub reliable_index: u32,
    pub sequence_index: u32,
    pub order_index: u32,
    pub channel: u8,
    pub split: Option<Split>,
    pub body: Vec<u8>,
}

fn put_u24(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes()[..3]);
}
fn get_u24(data: &[u8], at: usize) -> Option<u32> {
    let b = data.get(at..at + 3)?;
    Some(u32::from(b[0]) | u32::from(b[1]) << 8 | u32::from(b[2]) << 16)
}

impl Frame {
    pub fn encode(&self, out: &mut Vec<u8>) {
        out.push(self.reliability.id() << 5 | if self.split.is_some() { 0x10 } else { 0 });
        out.extend_from_slice(&((self.body.len() * 8) as u16).to_be_bytes());
        if self.reliability.reliable() {
            put_u24(out, self.reliable_index);
        }
        if self.reliability.sequenced() {
            put_u24(out, self.sequence_index);
        }
        if self.reliability.ordered() {
            put_u24(out, self.order_index);
            out.push(self.channel);
        }
        if let Some(s) = self.split {
            out.extend_from_slice(&s.count.to_be_bytes());
            out.extend_from_slice(&s.id.to_be_bytes());
            out.extend_from_slice(&s.index.to_be_bytes());
        }
        out.extend_from_slice(&self.body);
    }
    fn size(&self) -> usize {
        let mut n = Vec::new();
        Frame {
            body: Vec::new(),
            ..self.clone()
        }
        .encode(&mut n);
        n.len() + self.body.len()
    }
    /// One frame from the start of `data` and how many bytes it took.
    pub fn decode(data: &[u8]) -> Result<(Self, usize), DecodeError> {
        let short = DecodeError::Truncated;
        let flags = *data.first().ok_or(short)?;
        let reliability =
            Reliability::from_id(flags >> 5).ok_or(DecodeError::Malformed("reliability"))?;
        let bits = u16::from_be_bytes(data.get(1..3).ok_or(short)?.try_into().unwrap());
        let len = usize::from(bits).div_ceil(8);
        let mut at = 3;
        let mut frame = Frame {
            reliability,
            reliable_index: 0,
            sequence_index: 0,
            order_index: 0,
            channel: 0,
            split: None,
            body: Vec::new(),
        };
        if reliability.reliable() {
            frame.reliable_index = get_u24(data, at).ok_or(short)?;
            at += 3;
        }
        if reliability.sequenced() {
            frame.sequence_index = get_u24(data, at).ok_or(short)?;
            at += 3;
        }
        if reliability.ordered() {
            frame.order_index = get_u24(data, at).ok_or(short)?;
            frame.channel = *data.get(at + 3).ok_or(short)?;
            at += 4;
        }
        if flags & 0x10 != 0 {
            let b = data.get(at..at + 10).ok_or(short)?;
            frame.split = Some(Split {
                count: u32::from_be_bytes(b[0..4].try_into().unwrap()),
                id: u16::from_be_bytes(b[4..6].try_into().unwrap()),
                index: u32::from_be_bytes(b[6..10].try_into().unwrap()),
            });
            at += 10;
        }
        frame.body = data.get(at..at + len).ok_or(short)?.to_vec();
        Ok((frame, at + len))
    }
}

/// Sequence numbers an acknowledgement (or negative one) names, as the format's single numbers and ranges.
pub fn encode_ranges(flag: u8, numbers: &BTreeSet<u32>) -> Vec<u8> {
    let mut ranges: Vec<(u32, u32)> = Vec::new();
    for &n in numbers {
        match ranges.last_mut() {
            Some(last) if last.1 + 1 == n => last.1 = n,
            _ => ranges.push((n, n)),
        }
    }
    let mut out = vec![flag];
    out.extend_from_slice(&(ranges.len() as u16).to_be_bytes());
    for (lo, hi) in ranges {
        if lo == hi {
            out.push(1);
            put_u24(&mut out, lo);
        } else {
            out.push(0);
            put_u24(&mut out, lo);
            put_u24(&mut out, hi);
        }
    }
    out
}

pub fn decode_ranges(datagram: &[u8]) -> Result<Vec<u32>, DecodeError> {
    let short = DecodeError::Truncated;
    let count = u16::from_be_bytes(datagram.get(1..3).ok_or(short)?.try_into().unwrap());
    let mut at = 3;
    let mut numbers = Vec::new();
    for _ in 0..count {
        let single = *datagram.get(at).ok_or(short)? == 1;
        let lo = get_u24(datagram, at + 1).ok_or(short)?;
        at += 4;
        let hi = if single {
            lo
        } else {
            let hi = get_u24(datagram, at).ok_or(short)?;
            at += 3;
            hi
        };
        // A range is at most what one datagram could ever name, so a hostile peer can't make us allocate for 16 million.
        if hi < lo || hi - lo > 8192 || numbers.len() > 65536 {
            return Err(DecodeError::Malformed("range"));
        }
        numbers.extend(lo..=hi);
    }
    Ok(numbers)
}

struct Sent {
    frames: Vec<Frame>,
    last: Instant,
}

#[derive(Default)]
struct Channel {
    /// The next order index to hand to the game.
    next: u32,
    held: BTreeMap<u32, Vec<u8>>,
    /// Highest sequence index delivered, for sequenced frames: older ones are dropped.
    sequence: Option<u32>,
}

/// One end of a RakNet connection.
pub struct Session {
    mtu: usize,
    next_set: u32,
    next_reliable: u32,
    next_order: [u32; CHANNELS],
    next_sequence: u32,
    next_split: u16,
    queue: VecDeque<Frame>,
    sent: BTreeMap<u32, Sent>,
    /// When each unacknowledged reliable frame (by its reliable index) first went out, across resends.
    first_sent: HashMap<u32, Instant>,
    to_ack: BTreeSet<u32>,
    to_nack: BTreeSet<u32>,
    highest_seen: Option<u32>,
    seen_reliable: BTreeSet<u32>,
    reliable_floor: u32,
    channels: Vec<Channel>,
    splits: HashMap<u16, Vec<Option<Vec<u8>>>>,
    dead: bool,
}

impl Session {
    /// `mtu` is the size the handshake agreed, IP and UDP headers included.
    pub fn new(mtu: u16) -> Self {
        Self {
            mtu: usize::from(mtu),
            next_set: 0,
            next_reliable: 0,
            next_order: [0; CHANNELS],
            next_sequence: 0,
            next_split: 0,
            queue: VecDeque::new(),
            sent: BTreeMap::new(),
            first_sent: HashMap::new(),
            to_ack: BTreeSet::new(),
            to_nack: BTreeSet::new(),
            highest_seen: None,
            seen_reliable: BTreeSet::new(),
            reliable_floor: 0,
            channels: (0..CHANNELS).map(|_| Channel::default()).collect(),
            splits: HashMap::new(),
            dead: false,
        }
    }

    /// The most a single frame's body may hold before the payload is split.
    fn body_room(&self) -> usize {
        self.mtu - crate::raknet::HEADERS - SET_HEADER - FRAME_HEADER_MAX
    }

    /// Queues `payload`, split when it would not fit one datagram. Sequenced kinds are never split by RakNet's rules
    /// of use here: they are for small, replaceable messages, and are sent whole or refused.
    pub fn send(&mut self, payload: &[u8], reliability: Reliability, channel: u8) {
        let channel = channel.min(CHANNELS as u8 - 1);
        let room = self.body_room();
        let order_index = if reliability.ordered() && !reliability.sequenced() {
            let i = self.next_order[usize::from(channel)];
            self.next_order[usize::from(channel)] = (i + 1) & 0xff_ffff;
            i
        } else if reliability.sequenced() {
            self.next_order[usize::from(channel)]
        } else {
            0
        };
        let sequence_index = if reliability.sequenced() {
            let i = self.next_sequence;
            self.next_sequence = (i + 1) & 0xff_ffff;
            i
        } else {
            0
        };
        let pieces: Vec<&[u8]> = if payload.len() <= room {
            vec![payload]
        } else if reliability.sequenced() {
            return;
        } else {
            payload.chunks(room).collect()
        };
        let split_id = self.next_split;
        if pieces.len() > 1 {
            self.next_split = self.next_split.wrapping_add(1);
        }
        let count = pieces.len() as u32;
        for (index, piece) in pieces.into_iter().enumerate() {
            let reliable_index = if reliability.reliable() {
                let i = self.next_reliable;
                self.next_reliable = (i + 1) & 0xff_ffff;
                i
            } else {
                0
            };
            self.queue.push_back(Frame {
                reliability,
                reliable_index,
                sequence_index,
                order_index,
                channel,
                split: (count > 1).then_some(Split {
                    count,
                    id: split_id,
                    index: index as u32,
                }),
                body: piece.to_vec(),
            });
        }
    }

    /// The datagrams to put on the wire now: acknowledgements owed, queued frames and resends.
    pub fn flush(&mut self, now: Instant) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        if !self.to_ack.is_empty() {
            out.push(encode_ranges(FLAG_VALID | FLAG_ACK, &self.to_ack));
            self.to_ack.clear();
        }
        if !self.to_nack.is_empty() {
            out.push(encode_ranges(FLAG_VALID | FLAG_NACK, &self.to_nack));
            self.to_nack.clear();
        }
        // Frames that waited too long go round again, in a new set: the old number stays unacknowledged.
        let stale: Vec<u32> = self
            .sent
            .iter()
            .filter(|(_, s)| now.duration_since(s.last) >= RESEND_AFTER)
            .map(|(n, _)| *n)
            .collect();
        for n in stale {
            if let Some(s) = self.sent.remove(&n) {
                let oldest = s
                    .frames
                    .iter()
                    .filter_map(|f| self.first_sent.get(&f.reliable_index))
                    .min();
                if oldest.is_some_and(|t| now.duration_since(*t) >= GIVE_UP_AFTER) {
                    self.dead = true;
                    continue;
                }
                for f in s.frames {
                    self.queue.push_front(f);
                }
            }
        }
        let limit = self.mtu - crate::raknet::HEADERS;
        while !self.queue.is_empty() {
            let number = self.next_set;
            self.next_set = (number + 1) & 0xff_ffff;
            let mut datagram = vec![FLAGS_FRAME_SET];
            put_u24(&mut datagram, number);
            let mut frames = Vec::new();
            while let Some(f) = self.queue.front() {
                if datagram.len() + f.size() > limit && !frames.is_empty() {
                    break;
                }
                let f = self.queue.pop_front().unwrap();
                f.encode(&mut datagram);
                frames.push(f);
            }
            // Only reliable frames come back after a loss.
            let keep: Vec<Frame> = frames
                .into_iter()
                .filter(|f| f.reliability.reliable())
                .collect();
            if !keep.is_empty() {
                for f in &keep {
                    self.first_sent.entry(f.reliable_index).or_insert(now);
                }
                self.sent.insert(
                    number,
                    Sent {
                        frames: keep,
                        last: now,
                    },
                );
            }
            out.push(datagram);
        }
        out
    }

    /// Whether the peer stopped answering: a datagram went unacknowledged for [`GIVE_UP_AFTER`].
    pub fn is_dead(&self) -> bool {
        self.dead
    }

    /// Feeds in one datagram; returns the payloads now ready for the game, in order.
    pub fn receive(&mut self, datagram: &[u8], now: Instant) -> Result<Vec<Vec<u8>>, DecodeError> {
        let flags = *datagram.first().ok_or(DecodeError::Truncated)?;
        if flags & FLAG_VALID == 0 {
            return Err(DecodeError::Malformed("not a connected datagram"));
        }
        if flags & FLAG_ACK != 0 {
            for n in decode_ranges(datagram)? {
                for f in self.sent.remove(&n).into_iter().flat_map(|s| s.frames) {
                    self.first_sent.remove(&f.reliable_index);
                }
            }
            return Ok(Vec::new());
        }
        if flags & FLAG_NACK != 0 {
            for n in decode_ranges(datagram)? {
                if let Some(s) = self.sent.remove(&n) {
                    for f in s.frames.into_iter().rev() {
                        self.queue.push_front(f);
                    }
                }
            }
            let _ = now;
            return Ok(Vec::new());
        }
        let number = get_u24(datagram, 1).ok_or(DecodeError::Truncated)?;
        self.to_ack.insert(number);
        self.to_nack.remove(&number);
        if let Some(high) = self.highest_seen {
            if number > high + 1 {
                // Whatever fell in the gap and has not shown up is asked for again.
                self.to_nack.extend((high + 1..number).take(512));
            }
        }
        self.highest_seen = Some(self.highest_seen.map_or(number, |h| h.max(number)));
        let mut payloads = Vec::new();
        let mut at = SET_HEADER;
        while at < datagram.len() {
            let (frame, used) = Frame::decode(&datagram[at..])?;
            at += used;
            self.accept(frame, &mut payloads);
        }
        Ok(payloads)
    }

    fn accept(&mut self, frame: Frame, out: &mut Vec<Vec<u8>>) {
        if frame.reliability.reliable() {
            // A reliable frame is handed over once, however often it is resent.
            if frame.reliable_index < self.reliable_floor
                || !self.seen_reliable.insert(frame.reliable_index)
            {
                return;
            }
            while self.seen_reliable.remove(&self.reliable_floor) {
                self.reliable_floor = (self.reliable_floor + 1) & 0xff_ffff;
            }
        }
        let body = match frame.split {
            None => frame.body.clone(),
            Some(split) => {
                if split.count == 0 || split.count > 8192 || split.index >= split.count {
                    return;
                }
                let pieces = self
                    .splits
                    .entry(split.id)
                    .or_insert_with(|| vec![None; split.count as usize]);
                if pieces.len() != split.count as usize {
                    return;
                }
                pieces[split.index as usize] = Some(frame.body.clone());
                if pieces.iter().any(Option::is_none) {
                    return;
                }
                let whole: Vec<u8> = self
                    .splits
                    .remove(&split.id)
                    .unwrap()
                    .into_iter()
                    .flatten()
                    .flatten()
                    .collect();
                whole
            }
        };
        let channel = &mut self.channels[usize::from(frame.channel).min(CHANNELS - 1)];
        match frame.reliability {
            Reliability::Unreliable | Reliability::Reliable => out.push(body),
            Reliability::UnreliableSequenced | Reliability::ReliableSequenced => {
                if channel.sequence.is_none_or(|s| frame.sequence_index > s) {
                    channel.sequence = Some(frame.sequence_index);
                    out.push(body);
                }
            }
            Reliability::ReliableOrdered => {
                if frame.order_index < channel.next {
                    return;
                }
                channel.held.insert(frame.order_index, body);
                while let Some(b) = channel.held.remove(&channel.next) {
                    out.push(b);
                    channel.next = (channel.next + 1) & 0xff_ffff;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pair() -> (Session, Session) {
        (Session::new(1400), Session::new(1400))
    }
    /// Moves everything `from` has to say into `to`, dropping the datagrams `drop` picks (by position).
    fn pump(from: &mut Session, to: &mut Session, now: Instant, drop: &[usize]) -> Vec<Vec<u8>> {
        let mut got = Vec::new();
        for (i, d) in from.flush(now).into_iter().enumerate() {
            if drop.contains(&i) {
                continue;
            }
            got.extend(to.receive(&d, now).unwrap());
        }
        got
    }

    #[test]
    fn a_frame_round_trips_with_every_field() {
        let f = Frame {
            reliability: Reliability::ReliableOrdered,
            reliable_index: 0x010203,
            sequence_index: 0,
            order_index: 77,
            channel: 3,
            split: Some(Split {
                count: 4,
                id: 9,
                index: 2,
            }),
            body: vec![1, 2, 3, 4, 5],
        };
        let mut out = Vec::new();
        f.encode(&mut out);
        assert_eq!(out.len(), f.size());
        assert_eq!(Frame::decode(&out).unwrap(), (f, out.len()));
        assert!(Frame::decode(&out[..out.len() - 1]).is_err());
    }

    #[test]
    fn ranges_round_trip_and_reject_huge_ones() {
        let set: BTreeSet<u32> = [1, 2, 3, 7, 9, 10].into();
        let bytes = encode_ranges(FLAG_VALID | FLAG_ACK, &set);
        assert_eq!(
            decode_ranges(&bytes).unwrap(),
            set.into_iter().collect::<Vec<_>>()
        );
        // 0 .. 0xffffff in one range is refused rather than allocated.
        let evil = [0xc0, 0, 1, 0, 0, 0, 0, 0xff, 0xff, 0xff];
        assert!(decode_ranges(&evil).is_err());
    }

    #[test]
    fn reliable_ordered_arrives_in_order_and_is_acknowledged() {
        let (mut a, mut b) = pair();
        let now = Instant::now();
        a.send(b"one", Reliability::ReliableOrdered, 0);
        a.send(b"two", Reliability::ReliableOrdered, 0);
        assert_eq!(
            pump(&mut a, &mut b, now, &[]),
            vec![b"one".to_vec(), b"two".to_vec()]
        );
        // b answers with an acknowledgement, after which a has nothing left to resend.
        pump(&mut b, &mut a, now, &[]);
        assert!(a.sent.is_empty());
        assert!(a.flush(now + RESEND_AFTER * 2).is_empty());
    }

    #[test]
    fn a_lost_datagram_is_sent_again_after_the_wait() {
        let (mut a, mut b) = pair();
        let t0 = Instant::now();
        a.send(b"lost once", Reliability::Reliable, 0);
        assert!(pump(&mut a, &mut b, t0, &[0]).is_empty());
        assert!(a.flush(t0 + RESEND_AFTER / 2).is_empty());
        let got = pump(&mut a, &mut b, t0 + RESEND_AFTER, &[]);
        assert_eq!(got, vec![b"lost once".to_vec()]);
    }

    #[test]
    fn a_gap_is_reported_and_the_frame_comes_again_at_once() {
        let (mut a, mut b) = pair();
        let t0 = Instant::now();
        // Three datagrams, the middle one lost: b names it, and a resends it without the wait.
        for word in ["a", "b", "c"] {
            a.send(word.as_bytes(), Reliability::ReliableOrdered, 0);
            let ds = a.flush(t0);
            if word != "b" {
                for d in ds {
                    b.receive(&d, t0).unwrap();
                }
            }
        }
        let back = b.flush(t0);
        let mut nacked = false;
        for d in &back {
            nacked |= d[0] & FLAG_NACK != 0;
            a.receive(d, t0).unwrap();
        }
        assert!(nacked);
        let got = pump(&mut a, &mut b, t0, &[]);
        assert_eq!(got, vec![b"b".to_vec(), b"c".to_vec()]);
    }

    #[test]
    fn a_resent_reliable_frame_is_delivered_once() {
        let (mut a, mut b) = pair();
        let t0 = Instant::now();
        a.send(b"once", Reliability::Reliable, 0);
        let first = a.flush(t0);
        assert_eq!(b.receive(&first[0], t0).unwrap().len(), 1);
        // The acknowledgement is lost, a sends again, and b drops the copy.
        let again = a.flush(t0 + RESEND_AFTER);
        assert!(b.receive(&again[0], t0).unwrap().is_empty());
    }

    #[test]
    fn a_big_packet_is_split_and_put_together() {
        let (mut a, mut b) = pair();
        let now = Instant::now();
        let big: Vec<u8> = (0..20_000u32).map(|i| (i % 251) as u8).collect();
        a.send(&big, Reliability::ReliableOrdered, 0);
        a.send(b"after", Reliability::ReliableOrdered, 0);
        let got = pump(&mut a, &mut b, now, &[]);
        assert_eq!(got, vec![big, b"after".to_vec()]);
    }

    #[test]
    fn ordered_frames_wait_for_the_one_before() {
        let (mut a, mut b) = pair();
        let now = Instant::now();
        let mut datagrams = Vec::new();
        for word in ["1", "2", "3"] {
            a.send(word.as_bytes(), Reliability::ReliableOrdered, 0);
            datagrams.extend(a.flush(now));
        }
        // Delivered 3, 1, 2: nothing is handed over until 1 is here, then all three at once.
        assert!(b.receive(&datagrams[2], now).unwrap().is_empty());
        assert_eq!(b.receive(&datagrams[0], now).unwrap(), vec![b"1".to_vec()]);
        assert_eq!(
            b.receive(&datagrams[1], now).unwrap(),
            vec![b"2".to_vec(), b"3".to_vec()]
        );
    }

    #[test]
    fn an_old_sequenced_frame_is_dropped() {
        let (mut a, mut b) = pair();
        let now = Instant::now();
        a.send(b"old", Reliability::UnreliableSequenced, 0);
        let old = a.flush(now);
        a.send(b"new", Reliability::UnreliableSequenced, 0);
        let new = a.flush(now);
        assert_eq!(b.receive(&new[0], now).unwrap(), vec![b"new".to_vec()]);
        assert!(b.receive(&old[0], now).unwrap().is_empty());
    }

    #[test]
    fn a_silent_peer_is_given_up_on() {
        let (mut a, _) = pair();
        let t0 = Instant::now();
        a.send(b"hello?", Reliability::Reliable, 0);
        a.flush(t0);
        let mut t = t0;
        while t < t0 + GIVE_UP_AFTER + RESEND_AFTER * 2 {
            t += RESEND_AFTER;
            a.flush(t);
        }
        assert!(a.is_dead());
    }

    #[test]
    fn garbage_is_an_error_not_a_panic() {
        let mut b = Session::new(1400);
        let now = Instant::now();
        for bad in [
            &[][..],
            &[0x00],
            &[0x84, 1],
            &[0x84, 0, 0, 0, 0xff, 0xff],
            &[0xc0, 0, 5],
        ] {
            assert!(b.receive(bad, now).is_err(), "{bad:?}");
        }
    }
}
