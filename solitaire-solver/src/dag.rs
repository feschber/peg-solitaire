use std::{
    collections::{HashMap, HashSet, hash_map::Entry},
    fmt::Display,
};

use crate::board::Board;

/// directed acyclic graph to represent a solution graph
/// each node represents a board state, and each branch a possible move

pub(crate) struct SolutionDag {
    elements: HashMap<Board, Option<HashSet<Board>>>,
    root: Board,
}

impl SolutionDag {
    pub(crate) fn len(&self) -> usize {
        self.elements.len()
    }

    pub(crate) fn new(root: Board) -> Self {
        let elements = Default::default();
        Self { elements, root }
    }

    pub(crate) fn solutions(&mut self, board: Board) -> Option<Option<HashSet<Board>>> {
        self.elements.get(&board).cloned()
    }

    pub(crate) fn add_solution(&mut self, parent: Board, board: Board) {
        match self.elements.entry(parent) {
            Entry::Occupied(mut occupied_entry) => {
                occupied_entry.get_mut().as_mut().unwrap().insert(board);
            }
            Entry::Vacant(vacant_entry) => {
                vacant_entry.insert(Some(HashSet::from_iter([board])));
            }
        }
    }

    pub(crate) fn no_solution(&mut self, board: Board) {
        self.elements.insert(board, None);
    }
}

impl Display for SolutionDag {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "graph solution {{")?;
        for node in self.elements.iter() {
            if node.0.count_balls() > 8 {
                continue;
            }
            if node.1.is_none() {
                continue;
            }
            let style = match node.1 {
                Some(_) => "style=filled, fillcolor=green",
                None => "style=filled, fillcolor=red",
            };
            writeln!(
                f,
                "n{} [label=\"{}\", {style}];",
                node.0.0,
                node.0.to_string().replace("\n", "\\n")
            )?;
        }
        for node in self.elements.iter() {
            if node.0.count_balls() > 8 {
                continue;
            }
            if let (b, Some(n)) = node {
                for n in n {
                    writeln!(f, "n{} -- n{};", b.0, n.0)?;
                }
            }
        }
        writeln!(f, "}}")?;
        Ok(())
    }
}
