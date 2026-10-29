use std::num::NonZero;

use rayon::prelude::*;

pub(crate) fn num_threads() -> NonZero<usize> {
    std::thread::available_parallelism().unwrap_or(NonZero::new(4).unwrap())
}

/// configures the global rayon thread pool to use exactly `nthreads` worker threads.
///
/// safe to call more than once per process (a pool that's already built is left
/// as-is); callers that want an explicit `--threads` override honored should call
/// this once, before any of the other functions in this module run.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn configure_thread_pool(nthreads: usize) {
    let _ = rayon::ThreadPoolBuilder::new()
        .num_threads(nthreads)
        .build_global();
}

/// maps chunks of a slice `&[T]` into `R` in parallel using F.
///
/// chunks are intentionally smaller than `len / nthreads` (several chunks per
/// thread) and dispatched through rayon's work-stealing scheduler rather than
/// one fixed contiguous span per thread: per-board move counts vary, so a
/// static one-chunk-per-thread split can leave some threads idle while others
/// are still working through a heavier chunk.
fn par_map_chunks<F, T, R>(t: impl AsRef<[T]>, nthreads: usize, f: F) -> Vec<R>
where
    T: Send + Sync,
    F: Fn(&[T]) -> R + Send + Sync,
    R: Default + Send + Sync,
{
    let slice = t.as_ref();
    if nthreads == 1 || slice.len() < 100 * nthreads {
        return vec![f(slice)];
    }
    let chunk_size = slice.len().div_ceil(nthreads * 2);
    slice.par_chunks(chunk_size).map(f).collect()
}

/// slices `v` into multiple mutable slices according to `lens` lengths
pub(crate) fn into_mut_slices<'a, T>(mut v: &'a mut [T], lens: &[usize]) -> Vec<&'a mut [T]> {
    let mut slices = vec![];
    assert_eq!(v.len(), lens.iter().sum());
    for len in lens {
        let (a, b) = v.split_at_mut(*len);
        slices.push(a);
        v = b;
    }
    slices
}

/// [`par_join`] for chunks the caller owns: a lone chunk is handed back as-is instead of
/// being copied into a fresh allocation.
///
/// Worth a separate entry point because one chunk is the *common* case at both ends of a run,
/// not an edge case. [`par_map_chunks`] returns exactly one whenever the work is
/// single-threaded or too small to be worth splitting (`len < 100 * nthreads`), and that
/// second condition covers the many short rounds near the start and end of the BFS even on 16
/// threads. Joining a lone chunk allocates a second full-size buffer and copies every element
/// into it, to arrive at precisely the `Vec` that was already sitting there.
///
/// The multi-chunk path is unchanged - it still needs the parallel copy-out, because the
/// chunks are separate allocations and the result has to be contiguous.
pub(crate) fn par_join_owned<T: Copy + Send + Sync>(mut chunks: Vec<Vec<T>>) -> Vec<T> {
    match chunks.len() {
        0 => Vec::new(),
        1 => chunks.pop().expect("just checked there is one"),
        _ => par_join(&chunks),
    }
}

pub(crate) fn par_join<T: Copy + Send + Sync, VT: Send + Sync + AsRef<[T]>>(
    slices: &[VT],
) -> Vec<T> {
    let lens = slices.iter().map(|r| r.as_ref().len()).collect::<Vec<_>>();
    let total = lens.iter().sum();
    let mut result = Vec::with_capacity(total);
    let uninit = result.spare_capacity_mut();
    let dsts = into_mut_slices(uninit, &lens);
    // dispatched on rayon's already-live global pool (see `configure_thread_pool`)
    // instead of `thread::scope`: this function is called many times per BFS
    // round, and spawning fresh OS threads on every call (as `thread::scope` does)
    // adds up across a run to a lot of short-lived thread creations, which turned
    // out to be the dominant source of process overhead, not anything algorithmic.
    dsts.into_par_iter()
        .zip(slices.par_iter())
        .for_each(|(dst, src)| {
            let dst: &mut [T] = unsafe { std::mem::transmute(dst) };
            dst.copy_from_slice(src.as_ref());
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
    par_join_owned(par_map_chunks(states, nthreads, f))
}
