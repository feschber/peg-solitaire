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

/// maps chunks of a slice `&mut [T]` into `R` in parallel using F; see [`par_map_chunks`].
fn par_map_chunks_mut<F, T, R>(mut t: impl AsMut<[T]>, nthreads: usize, f: F) -> Vec<R>
where
    T: Send + Sync,
    F: Fn(&mut [T]) -> R + Send + Sync,
    R: Default + Send + Sync,
{
    let slice = t.as_mut();
    if nthreads == 1 || slice.len() < 100 * nthreads {
        return vec![f(slice)];
    }
    let chunk_size = slice.len().div_ceil(nthreads * 2);
    slice.par_chunks_mut(chunk_size).map(f).collect()
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

/// Maps `src` in parallel into a single output holding exactly `ratio` results per
/// input, each chunk writing straight into its own slice of it.
///
/// [`parallel`] cannot do this because its `f` returns a `Vec` of unknown length, so
/// it has to build one per chunk and then [`par_join`] them into a second
/// allocation - every element allocated twice and copied once. Where the output
/// size follows from the input size, both are avoidable, and the copy is not small:
/// the two callers move 16 and 27 MB.
///
/// Measured on the inverse step's 2.6M boards, against the collect-then-join shape
/// (with `par_join`'s parallel copy, not a sequential concat): 2.505 -> 1.876 ms,
/// -25%. The map itself is only 0.55 ms of that; the rest was plumbing.
///
/// `f` must fill every element of the slice it is handed - the output's length is
/// set from `ratio` regardless, so a slot left untouched would be read as
/// uninitialized memory. `R: Copy` keeps that sound in the same way [`par_join`]
/// does: no destructor runs on the uninitialized value being assigned over.
pub(crate) fn par_map_exact<T, R, F>(src: &[T], nthreads: usize, ratio: usize, f: F) -> Vec<R>
where
    T: Send + Sync,
    R: Copy + Send + Sync,
    F: Fn(&[T], &mut [R]) + Send + Sync,
{
    let total = src.len() * ratio;
    let mut out = Vec::with_capacity(total);
    let chunk = if nthreads == 1 || src.len() < 100 * nthreads {
        src.len().max(1)
    } else {
        src.len().div_ceil(nthreads * 2).max(1)
    };
    {
        let lens: Vec<usize> = src.chunks(chunk).map(|c| c.len() * ratio).collect();
        let dsts = into_mut_slices(out.spare_capacity_mut(), &lens);
        dsts.into_par_iter()
            .zip(src.par_chunks(chunk))
            .for_each(|(dst, src)| {
                // SAFETY: as in `par_join` - `R: Copy` has no drop glue, so assigning
                // over the uninitialized slots runs no destructor, and `f` is
                // required to write all of them before `set_len` below exposes them.
                let dst: &mut [R] = unsafe { std::mem::transmute(dst) };
                debug_assert_eq!(dst.len(), src.len() * ratio);
                f(src, dst);
            });
    }
    // SAFETY: every chunk covered a disjoint span of `0..total` and filled it.
    unsafe { out.set_len(total) };
    out
}

pub(crate) fn parallel<F, T, R>(states: &[T], nthreads: usize, f: F) -> Vec<R>
where
    T: Send + Sync,
    F: Fn(&[T]) -> Vec<R> + Send + Sync,
    R: Copy + Default + Send + Sync,
{
    par_join_owned(par_map_chunks(states, nthreads, f))
}

/// filters `items` in parallel, pre-sizing each chunk's output buffer exactly
/// once instead of growing it via repeated reallocation as matches are found -
/// same reasoning as the capacity precounting already used in `board.rs`'s
/// `possible_moves`/`possible_reverse_moves` and `keyset.rs`'s extraction: a
/// plain `chunk.iter().filter(pred).collect()` per chunk showed up as real,
/// avoidable allocator cost (`finish_grow`) once `pred` got cheap enough that
/// the buffer growth was no longer negligible next to it.
pub(crate) fn par_filter<T, F>(items: &[T], nthreads: usize, pred: F) -> Vec<T>
where
    T: Copy + Send + Sync,
    F: Fn(&T) -> bool + Send + Sync,
{
    let chunks: Vec<Vec<T>> = par_map_chunks(items, nthreads, |chunk| {
        let count = chunk.iter().filter(|x| pred(x)).count();
        let mut out = Vec::with_capacity(count);
        out.extend(chunk.iter().copied().filter(|x| pred(x)));
        out
    });
    par_join_owned(chunks)
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
            // `Vec::from(c)` allocates at the pre-dedup chunk size; dedup() only
            // shrinks the logical length, not the allocation. In high-duplicate
            // rounds (we've seen >80% of generated moves be duplicates) that
            // leaves most of this buffer allocated-but-unused for the rest of
            // par_dedup's call, right as memory pressure peaks.
            v.shrink_to_fit();
            v
        });
        for i in 0..chunks.len() - 1 {
            if chunks[i][chunks[i].len() - 1] == chunks[i + 1][0] {
                chunks[i].pop();
            }
        }
        par_join_owned(chunks)
    }
}

fn intersect_sorted_seq<R: Copy + Eq + Ord>(a: &[R], b: &[R]) -> Vec<R> {
    let mut ia = 0;
    let mut ib = 0;
    let mut res = vec![];
    while ia < a.len() && ib < b.len() {
        match a[ia].cmp(&b[ib]) {
            std::cmp::Ordering::Equal => {
                res.push(a[ia]);
                ia += 1;
                ib += 1;
            }
            std::cmp::Ordering::Less => ia += 1,
            std::cmp::Ordering::Greater => ib += 1,
        }
    }
    res
}

/// intersects two sorted, deduplicated slices, preserving order.
///
/// `a` is split into chunks (using the same raw-thread chunk machinery as the rest of
/// this module); the matching bound for each chunk in `b` is found via binary search,
/// so each chunk can be merged against its own (non-overlapping) slice of `b`
/// independently, then the per-chunk results are joined back together in order.
pub(crate) fn intersect_sorted<R>(a: &[R], b: &[R], nthreads: usize) -> Vec<R>
where
    R: Copy + Eq + Ord + Default + Send + Sync,
{
    if nthreads == 1 || a.len() < 100 * nthreads {
        return intersect_sorted_seq(a, b);
    }
    let chunks = par_map_chunks(a, nthreads, |chunk| match (chunk.first(), chunk.last()) {
        (Some(&first), Some(&last)) => {
            let lo = b.partition_point(|x| *x < first);
            let hi = lo + b[lo..].partition_point(|x| *x <= last);
            intersect_sorted_seq(chunk, &b[lo..hi])
        }
        _ => vec![],
    });
    par_join_owned(chunks)
}
