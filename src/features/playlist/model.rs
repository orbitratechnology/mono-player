//! The session queue: ordering, current item, repeat and shuffle. Independent of the engine.

use serde::{Deserialize, Serialize};

use crate::playback::MediaSource;

/// Stable identity of a queue entry (never a row index).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct QueueItemId(pub u64);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueueItem {
    pub id: QueueItemId,
    pub source: MediaSource,
    pub title: String,
    /// Set when the item failed to play, so the list can mark it without removing it.
    pub failed: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepeatMode {
    #[default]
    Off,
    All,
    One,
}

impl RepeatMode {
    pub fn cycle(self) -> Self {
        match self {
            Self::Off => Self::All,
            Self::All => Self::One,
            Self::One => Self::Off,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "Repeat off",
            Self::All => "Repeat all",
            Self::One => "Repeat one",
        }
    }
}

/// Why the queue is advancing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Advance {
    /// The user pressed Next/Previous: repeat-one does not trap them on one item.
    User,
    /// The current item ended on its own.
    Automatic,
}

#[derive(Debug, Default)]
pub struct Playlist {
    items: Vec<QueueItem>,
    current: Option<QueueItemId>,
    repeat: RepeatMode,
    shuffle: bool,
    /// Play order while shuffled. Always a permutation of `items`' ids.
    order: Vec<QueueItemId>,
    next_id: u64,
    rng: u64,
}

impl Playlist {
    pub fn new() -> Self {
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x9E37_79B9_7F4A_7C15);
        Self::with_seed(seed)
    }

    /// A deterministic playlist for tests.
    pub fn with_seed(seed: u64) -> Self {
        Self {
            rng: seed | 1,
            ..Default::default()
        }
    }

    pub fn items(&self) -> &[QueueItem] {
        &self.items
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn current(&self) -> Option<&QueueItem> {
        self.current.and_then(|id| self.get(id))
    }

    pub fn current_id(&self) -> Option<QueueItemId> {
        self.current
    }

    pub fn get(&self, id: QueueItemId) -> Option<&QueueItem> {
        self.items.iter().find(|item| item.id == id)
    }

    pub fn index_of(&self, id: QueueItemId) -> Option<usize> {
        self.items.iter().position(|item| item.id == id)
    }

    pub fn repeat(&self) -> RepeatMode {
        self.repeat
    }

    pub fn set_repeat(&mut self, repeat: RepeatMode) {
        self.repeat = repeat;
    }

    pub fn shuffle(&self) -> bool {
        self.shuffle
    }

    /// Turn shuffle on/off. Turning it on keeps the current item first in the new order.
    pub fn set_shuffle(&mut self, shuffle: bool) {
        self.shuffle = shuffle;
        if shuffle {
            self.reshuffle();
        } else {
            self.order.clear();
        }
    }

    /// Append sources; returns their new ids in order.
    pub fn add(&mut self, sources: impl IntoIterator<Item = MediaSource>) -> Vec<QueueItemId> {
        let mut added = Vec::new();
        for source in sources {
            self.next_id += 1;
            let id = QueueItemId(self.next_id);
            let title = source.display_name();
            self.items.push(QueueItem {
                id,
                source,
                title,
                failed: false,
            });
            if self.shuffle {
                // Insert somewhere after the current item so it is still reachable.
                let start = self
                    .current
                    .and_then(|c| self.order.iter().position(|o| *o == c))
                    .map_or(0, |p| p + 1);
                let span = (self.order.len() - start + 1) as u64;
                let at = start + (self.next_random() % span) as usize;
                self.order.insert(at, id);
            }
            added.push(id);
        }
        added
    }

    /// Remove an entry. If it was current, there is no current item afterwards.
    pub fn remove(&mut self, id: QueueItemId) -> bool {
        let Some(index) = self.index_of(id) else {
            return false;
        };
        self.items.remove(index);
        self.order.retain(|o| *o != id);
        if self.current == Some(id) {
            self.current = None;
        }
        true
    }

    pub fn clear(&mut self) {
        self.items.clear();
        self.order.clear();
        self.current = None;
    }

    /// Move an entry to `to` (clamped). Affects list order, not shuffle order.
    pub fn move_item(&mut self, id: QueueItemId, to: usize) -> bool {
        let Some(from) = self.index_of(id) else {
            return false;
        };
        let item = self.items.remove(from);
        let to = to.min(self.items.len());
        self.items.insert(to, item);
        true
    }

    pub fn set_current(&mut self, id: QueueItemId) -> bool {
        if self.get(id).is_some() {
            self.current = Some(id);
            true
        } else {
            false
        }
    }

    pub fn mark_failed(&mut self, id: QueueItemId, failed: bool) {
        if let Some(item) = self.items.iter_mut().find(|item| item.id == id) {
            item.failed = failed;
        }
    }

    /// The id that should play after the current one, without changing state.
    pub fn peek_next(&self, advance: Advance) -> Option<QueueItemId> {
        if self.items.is_empty() {
            return None;
        }
        let order = self.play_order();
        let Some(current) = self.current else {
            return order.first().copied();
        };
        if advance == Advance::Automatic && self.repeat == RepeatMode::One {
            return Some(current);
        }
        let position = order.iter().position(|id| *id == current)?;
        match order.get(position + 1) {
            Some(next) => Some(*next),
            None if self.repeat != RepeatMode::Off => order.first().copied(),
            None => None,
        }
    }

    /// The id before the current one, wrapping only when repeating.
    pub fn peek_previous(&self) -> Option<QueueItemId> {
        let order = self.play_order();
        let current = self.current?;
        let position = order.iter().position(|id| *id == current)?;
        if position > 0 {
            Some(order[position - 1])
        } else if self.repeat != RepeatMode::Off {
            order.last().copied()
        } else {
            None
        }
    }

    /// Advance and make the result current.
    pub fn advance(&mut self, advance: Advance) -> Option<QueueItemId> {
        let next = self.peek_next(advance)?;
        self.current = Some(next);
        Some(next)
    }

    pub fn go_previous(&mut self) -> Option<QueueItemId> {
        let previous = self.peek_previous()?;
        self.current = Some(previous);
        Some(previous)
    }

    fn play_order(&self) -> Vec<QueueItemId> {
        if self.shuffle {
            self.order.clone()
        } else {
            self.items.iter().map(|item| item.id).collect()
        }
    }

    fn reshuffle(&mut self) {
        let mut order: Vec<QueueItemId> = self
            .items
            .iter()
            .map(|item| item.id)
            .filter(|id| Some(*id) != self.current)
            .collect();
        // Fisher–Yates.
        for i in (1..order.len()).rev() {
            let j = (self.next_random() % (i as u64 + 1)) as usize;
            order.swap(i, j);
        }
        if let Some(current) = self.current {
            order.insert(0, current);
        }
        self.order = order;
    }

    /// xorshift64*: tiny, deterministic for tests, good enough for shuffling a queue.
    fn next_random(&mut self) -> u64 {
        let mut x = self.rng;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.rng = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::path::PathBuf;

    use super::*;

    fn sources(n: usize) -> Vec<MediaSource> {
        (0..n)
            .map(|i| MediaSource::File(PathBuf::from(format!("/m/{i}.mp3"))))
            .collect()
    }

    #[test]
    fn sequential_order_and_repeat() {
        let mut list = Playlist::with_seed(7);
        let ids = list.add(sources(3));
        assert_eq!(list.advance(Advance::User), Some(ids[0]));
        assert_eq!(list.advance(Advance::Automatic), Some(ids[1]));
        assert_eq!(list.advance(Advance::Automatic), Some(ids[2]));
        assert_eq!(list.advance(Advance::Automatic), None);

        list.set_repeat(RepeatMode::All);
        assert_eq!(list.advance(Advance::Automatic), Some(ids[0]));
        assert_eq!(list.peek_previous(), Some(ids[2]));

        list.set_repeat(RepeatMode::One);
        assert_eq!(list.advance(Advance::Automatic), Some(ids[0]));
        // The user can still skip.
        assert_eq!(list.advance(Advance::User), Some(ids[1]));
    }

    #[test]
    fn previous_without_repeat_stops_at_start() {
        let mut list = Playlist::with_seed(1);
        let ids = list.add(sources(2));
        list.set_current(ids[0]);
        assert_eq!(list.go_previous(), None);
        list.set_current(ids[1]);
        assert_eq!(list.go_previous(), Some(ids[0]));
    }

    #[test]
    fn remove_and_move_use_stable_ids() {
        let mut list = Playlist::with_seed(1);
        let ids = list.add(sources(4));
        list.set_current(ids[2]);
        assert!(list.move_item(ids[3], 0));
        assert_eq!(list.items()[0].id, ids[3]);
        assert_eq!(list.current_id(), Some(ids[2]));
        assert!(list.move_item(ids[3], 99));
        assert_eq!(list.items().last().unwrap().id, ids[3]);
        assert!(list.remove(ids[2]));
        assert_eq!(list.current_id(), None);
        assert!(!list.remove(ids[2]));
        assert_eq!(list.len(), 3);
    }

    #[test]
    fn shuffle_is_a_permutation_starting_at_current() {
        let mut list = Playlist::with_seed(42);
        let ids = list.add(sources(20));
        list.set_current(ids[5]);
        list.set_shuffle(true);
        let mut seen = vec![ids[5]];
        while let Some(next) = list.advance(Advance::Automatic) {
            seen.push(next);
        }
        assert_eq!(seen.len(), 20);
        assert_eq!(seen.iter().collect::<HashSet<_>>().len(), 20);
        assert_ne!(seen, {
            let mut sorted = seen.clone();
            sorted.sort();
            sorted
        });
    }

    #[test]
    fn items_added_while_shuffled_are_reachable() {
        let mut list = Playlist::with_seed(3);
        let first = list.add(sources(3));
        list.set_current(first[0]);
        list.set_shuffle(true);
        let added = list.add(sources(2));
        let mut seen = HashSet::new();
        while let Some(next) = list.advance(Advance::Automatic) {
            seen.insert(next);
        }
        assert!(added.iter().all(|id| seen.contains(id)));
        list.set_shuffle(false);
        assert_eq!(list.len(), 5);
    }

    #[test]
    fn empty_queue_has_nothing_next() {
        let mut list = Playlist::with_seed(1);
        assert_eq!(list.advance(Advance::User), None);
        assert_eq!(list.go_previous(), None);
        assert_eq!(RepeatMode::Off.cycle().cycle().cycle(), RepeatMode::Off);
    }
}
