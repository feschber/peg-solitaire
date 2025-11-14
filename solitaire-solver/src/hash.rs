// use ahash::AHashSet as HashSet; // 1.194s
// use fnv::FnvHashSet as HashSet; // 1.024s
// use rustc_hash::FxHashSet as HashSet; // 0.866s
// => FxHash using NoHashHasher;

use nohash_hasher::BuildNoHashHasher;
use std::collections::{HashMap, HashSet};

pub type CustomHashSet<V> = HashSet<V, BuildNoHashHasher<V>>;
pub type CustomHashMap<K, V> = HashMap<K, V, BuildNoHashHasher<K>>;
