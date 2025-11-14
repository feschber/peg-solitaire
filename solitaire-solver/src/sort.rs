// use rayon::slice::ParallelSliceMut;
use voracious_radix_sort::{RadixKey, RadixSort, Radixable};

pub trait Sort<T: Radixable<K>, K: RadixKey> {
    #[allow(unused)]
    fn fast_sort_unstable(&mut self);
    #[allow(unused)]
    fn fast_sort_unstable_mt(&mut self, threads: usize);
}

impl<T: Radixable<K>, K: RadixKey> Sort<T, K> for [T] {
    fn fast_sort_unstable(&mut self) {
        self.voracious_sort()
    }
    fn fast_sort_unstable_mt(&mut self, threads: usize) {
        if threads == 1 {
            self.voracious_sort()
        } else {
            self.voracious_mt_sort(threads)
        }
    }
}

impl<T: Radixable<K>, K: RadixKey> Sort<T, K> for Vec<T> {
    fn fast_sort_unstable(&mut self) {
        self.as_mut_slice().fast_sort_unstable();
    }
    fn fast_sort_unstable_mt(&mut self, threads: usize) {
        self.as_mut_slice().fast_sort_unstable_mt(threads);
    }
}
