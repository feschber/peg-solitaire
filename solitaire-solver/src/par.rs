use std::{num::NonZero, thread};

pub(crate) fn num_threads() -> NonZero<usize> {
    std::thread::available_parallelism().unwrap_or(NonZero::new(4).unwrap())
}

/// maps n chunks of a slice `&[T]` into `R` in parallel using F
fn par_map_chunks<F, T, R>(t: impl AsRef<[T]>, nthreads: usize, f: F) -> Vec<R>
where
    T: Send + Sync,
    F: Fn(&[T]) -> R + Send + Sync,
    R: Default + Send + Sync,
{
    if nthreads == 1 || t.as_ref().len() < 100 * nthreads {
        vec![f(t.as_ref())]
    } else {
        let mut chunks = t.as_ref().chunks(t.as_ref().len().div_ceil(nthreads));
        thread::scope(|s| {
            let first_chunk = chunks.next().unwrap();
            let threads: Vec<_> = chunks.map(|c| s.spawn(|| f(c))).collect();

            // execute on current thread
            let mut results = vec![f(first_chunk)];
            results.extend(threads.into_iter().map(|t| t.join().unwrap()));
            results
        })
    }
}

/// maps n chunks of a slice `&[T]` into `R` in parallel using F
fn par_map_chunks_mut<F, T, R>(mut t: impl AsMut<[T]>, nthreads: usize, f: F) -> Vec<R>
where
    T: Send + Sync,
    F: Fn(&mut [T]) -> R + Send + Sync,
    R: Default + Send + Sync,
{
    if nthreads == 1 || t.as_mut().len() < 100 * nthreads {
        vec![f(t.as_mut())]
    } else {
        let chunk_size = t.as_mut().len().div_ceil(nthreads);
        let mut chunks = t.as_mut().chunks_mut(chunk_size);
        thread::scope(|s| {
            let first_chunk = chunks.next().unwrap();
            let threads: Vec<_> = chunks.map(|c| s.spawn(|| f(c))).collect();

            // execute on current thread
            let mut results = vec![f(first_chunk)];
            results.extend(threads.into_iter().map(|t| t.join().unwrap()));
            results
        })
    }
}

/// slices `v` into multiple mutable slices according to `lens` lengths
fn into_mut_slices<'a, T>(mut v: &'a mut [T], lens: &[usize]) -> Vec<&'a mut [T]> {
    let mut slices = vec![];
    assert_eq!(v.len(), lens.iter().sum());
    for len in lens {
        let (a, b) = v.split_at_mut(*len);
        slices.push(a);
        v = b;
    }
    slices
}

fn par_join<T: Copy + Send + Sync, VT: Send + Sync + AsRef<[T]>>(slices: &[VT]) -> Vec<T> {
    let lens = slices.iter().map(|r| r.as_ref().len()).collect::<Vec<_>>();
    let total = lens.iter().sum();
    let mut result = Vec::with_capacity(total);
    let uninit = result.spare_capacity_mut();
    let dsts = into_mut_slices(uninit, &lens);
    thread::scope(|s| {
        dsts.into_iter()
            .zip(slices)
            .map(|(dst, src)| {
                let dst: &mut [T] = unsafe { std::mem::transmute(dst) };
                s.spawn(|| dst.copy_from_slice(src.as_ref()))
            })
            .for_each(|_| {});
    });
    unsafe { result.set_len(total) };
    result
}

pub(crate) fn parallel<F, T, R>(states: &[T], nthreads: usize, f: F) -> Vec<R>
where
    T: Send + Sync,
    F: Fn(&[T]) -> Vec<R> + Send + Sync,
    R: Copy + Default + Send + Sync,
{
    par_join(&par_map_chunks(states, nthreads, f))
}

pub(crate) trait ParDedup {
    fn par_dedup(self, n_threads: usize) -> Self;
}

#[cfg(target_arch = "wasm32")]
impl<T: Copy + std::fmt::Debug + Send + Sync + PartialEq> ParDedup for Vec<T> {
    fn par_dedup(mut self, nthreads: usize) -> Self {
        self.dedup();
        self
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl<T: Copy + std::fmt::Debug + Send + Sync + PartialEq> ParDedup for Vec<T> {
    fn par_dedup(mut self, nthreads: usize) -> Self {
        if nthreads == 1 {
            self.dedup();
            return self;
        }
        let mut chunks: Vec<Vec<T>> = par_map_chunks_mut(self, nthreads, |c| {
            let mut v = Vec::from(c);
            v.dedup();
            v
        });
        for i in 0..chunks.len() - 1 {
            if chunks[i][chunks[i].len() - 1] == chunks[i + 1][0] {
                chunks[i].pop();
            }
        }
        par_join(&chunks)
    }
}
