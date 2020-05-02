use bytes::{
    Bytes,
    Buf
};
use rand::{
    distributions::Distribution,
    Rng
};
use std::{
    cmp,
    collections::{
        HashMap,
        HashSet,
    },
    hash::{
        Hash,
        Hasher,
    },
    iter,
    sync::atomic::{
        AtomicUsize,
        Ordering,
    },
};

#[derive(Clone)]
struct ByteWindows<'a> {
    bytes: &'a Bytes,
    window_size: usize,
    current_idx: usize,
    finished: bool
}
impl<'a> ByteWindows<'a> {
    fn new(bytes: &'a Bytes, size: usize) -> Self {
        Self {
            bytes,
            window_size: size,
            current_idx: 0,
            finished: false
        }
    }
}
impl<'a> Iterator for ByteWindows<'a> {
    type Item = Bytes;
    fn next(&mut self) -> Option<Bytes> {
        if self.finished {
            return None;
        }
        let end = self.current_idx.checked_add(self.window_size)?;
        let bytes = self.bytes.slice(self.current_idx..cmp::min(self.bytes.len(), end));
        self.current_idx += 1;
        if end >= self.bytes.len() {
            self.finished = true;
        }
        Some(bytes)
    }
}

struct Weighted<T> {
    weight: AtomicUsize,
    value: T
}
impl<T> Weighted<T> {
    pub fn new(value: T) -> Self {
        Self {
            weight: AtomicUsize::new(0),
            value
        }
    }
    pub fn weight(&self) -> usize {
        self.weight.load(Ordering::Relaxed)
    }
    pub fn add(&self) {
        self.weight.fetch_add(1, Ordering::Relaxed);
    }
    pub fn get(&self) -> &T {
        &self.value
    }
}
// impl Hash and PartialEq to ignore the weight value so that inserting into
// a HashSet will use a value which already exists if possible
impl<T: Hash> Hash for Weighted<T> {
    fn hash<H: Hasher>(&self, f: &mut H) {
        <T as Hash>::hash(&self.value, f)
    }
}
impl<T: PartialEq> PartialEq for Weighted<T> {
    fn eq(&self, other: &Self) -> bool {
        self.value == other.value
    }
}
impl<T: Eq> Eq for Weighted<T> { }

struct WeightedSet<T> {
    values: HashSet<Weighted<T>>,
    total_size: usize,
}
impl<T: Hash + Eq> WeightedSet<T> {
    pub fn new() -> Self {
        Self {
            values: HashSet::new(),
            total_size: 0,
        }
    }
    pub fn insert(&mut self, value: T) {
        self.values.get_or_insert(Weighted::new(value)).add();
        self.total_size += 1;
    }
}
impl<T: Clone> Distribution<T> for WeightedSet<T> {
    fn sample<R: Rng + ?Sized>(&self, rng: &mut R) -> T {
        let selected = rng.gen_range(0, self.total_size);
        self.values.iter()
            .scan(0, |accum, value| {
                *accum += value.weight();
                Some((*accum, value))
            })
            .find(|&(accum, _)| accum >= selected)
            .map(|(_, value)| Clone::clone(value.get()))
            .expect("Called `sample` on an empty WeightedSet")
    }
}

pub struct Generator<'a, R> {
    values: &'a HashMap<Option<Bytes>, WeightedSet<Option<Bytes>>>,
    current_value: Option<Bytes>,
    initial_value: Option<Bytes>,
    rng: R
}
impl<'a, R: Rng> Iterator for Generator<'a, R> {
    type Item = u8;
    fn next(&mut self) -> Option<u8> {
        // We want all of the bytes of the initial value
        if let Some(init) = self.initial_value.as_mut() {
            if init.len() == 0 {
                self.initial_value = None;
            } else {
                let ret = init[0];
                init.advance(1);
                return Some(ret);
            }
        }
        self.current_value = self.values
            .get(&self.current_value)
            .and_then(|set| self.rng.sample(set));

        // For other values, we only care about the last byte
        self.current_value.as_ref().map(|n| n[n.len() - 1])
    }
}

pub struct Chain {
    values: HashMap<Option<Bytes>, WeightedSet<Option<Bytes>>>,
    chain_len: usize
}
impl Chain {
    pub fn new(len: usize) -> Self {
        Self {
            values: HashMap::new(),
            chain_len: len
        }
    }
    fn feed_inner(&mut self, bytes: Bytes) {
        if bytes.len() > 0 {
            // We want an iterator like so (for the string "abcde"):
            //
            // (None, "abc"), ("abc", "bcd"), ("bcd", "cde"), ("cde", None)
            //
            // To do this we start with an iterator over "abc", "bcd", "cde":

            let base_iter = ByteWindows::new(&bytes, self.chain_len).map(Option::Some);
            // Then we create one iterator which will go through those values,
            // and finish with None
            let wind_a = base_iter.clone().chain(iter::once(None));
            // Then we create another iterator which will start with None, then
            // go through the values
            let wind_b = iter::once(None).chain(base_iter);

            //Then we zip the two iterators together
            for (prev, next) in wind_b.zip(wind_a) {
                self.values.entry(prev).or_insert_with(WeightedSet::new).insert(next);
            }
        }
    }
    pub fn feed<T: Into<Bytes>>(&mut self, feeder: T) {
        self.feed_inner(feeder.into())
    }
    pub fn generator<'a, R: Rng>(&'a self, mut rng: R) -> Generator<'a, R> {
        let first = self.values
            .get(&None)
            .and_then(|set| set.sample(&mut rng));
        Generator {
            values: &self.values,
            current_value: first.clone(),
            initial_value: first,
            rng
        }
    }
}

