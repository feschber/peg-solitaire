use nohash_hasher::BuildNoHashHasher;
use std::collections::HashSet;

pub(crate) type CustomHashSet<V> = HashSet<V, BuildNoHashHasher<V>>;
