use bytes::Bytes;
use rand::{
    distributions::Distribution,
    Rng
};
use std::{
    cmp,
    collections::HashMap,
    hash::Hash,
    iter,
};

struct WeightedSet<T> {
    values: HashMap<T, usize>,
    total_size: usize,
}
impl<T: Hash + Eq> WeightedSet<T> {
    pub fn new() -> Self {
        Self {
            values: HashMap::new(),
            total_size: 0,
        }
    }
    pub fn insert(&mut self, value: T) {
        *self.values.entry(value).or_insert(0) += 1;
        self.total_size += 1;
    }
}
impl<T: Clone> Distribution<T> for WeightedSet<T> {
    fn sample<R: Rng + ?Sized>(&self, rng: &mut R) -> T {
        let selected = rng.gen_range(1..=self.total_size);
        self.values.iter()
            .scan(0, |accum, (value, weight)| {
                *accum += *weight;
                Some((*accum >= selected, value))
            })
            .find_map(|(is_next, value)| is_next.then(|| value.clone()))
            .expect("Called `sample` on an empty WeightedSet")
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
    pub fn feed<T: Into<Bytes>>(&mut self, feeder: T) {
        fn byte_windows(bytes: &Bytes, size: usize) -> impl Iterator<Item=Bytes> + '_ {
            // The idea here is to iterate between 0 and the last window's left
            // position and then slice the bytes for the window size
            //
            // We need to special case for the bytes being smaller than the
            // window size though - i.e. we need to iterate at least once, so
            // make sure that the iterator range goes to at least 1
            (0..=bytes.len().saturating_sub(size))
                .into_iter()
                // if the bytes are smaller than the window size, then doing
                // bytes[idx..idx + size] will overflow the buffer, so we need
                // to make sure that the slice we make is within bounds
                .map(move |idx| bytes.slice(idx..cmp::min(bytes.len(), idx + size)))
        }

        fn inner(this: &mut Chain, bytes: Bytes) {
            if !bytes.is_empty() {
                // We want an iterator like so (for the string "abcde"):
                //
                // (None, "abc"), ("abc", "bcd"), ("bcd", "cde"), ("cde", None)
                //
                // To do this we start with an iterator over "abc", "bcd", "cde"
                // which is the above byte windows iterator for the bytes
                //
                // Then we create one iterator which will go through those values,
                // and finish with None
                let wind_a = byte_windows(&bytes, this.chain_len).map(Option::Some).chain(iter::once(None));
                // Then we create another iterator which will start with None, then
                // go through the values
                let wind_b = iter::once(None).chain(byte_windows(&bytes, this.chain_len).map(Option::Some));

                //Then we zip the two iterators together
                for (prev, next) in wind_b.zip(wind_a) {
                    this.values.entry(prev).or_insert_with(WeightedSet::new).insert(next);
                }
            }
        }

        inner(self, feeder.into())
    }
    pub fn generator<'a, R: Rng + 'a>(&'a self, mut rng: R) -> impl Iterator<Item=u8> + 'a {
        let mut random_segment = move |base| self.values.get(&base).and_then(|set| rng.sample(set));

        let mut segments = iter::successors(random_segment(None), move |b| random_segment(Some(b.clone())));

        // Get all bytes of the first segment
        segments.next()
            .into_iter()
            .flatten()
            // For every other segment, just get the last character
            .chain(segments.map(|b| b[b.len() - 1]))
    }
}

