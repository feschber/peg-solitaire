use crate::Board;

pub struct RadixTree {
    nodes: Vec<Node>,
    len: usize,
}

struct Node {
    children: Vec<(u8, u32)>,
    terminal: bool,
}

pub struct RadixTreeIterator<'a> {
    tree: &'a RadixTree,
    stack: Vec<Frame>,
}

struct Frame {
    node: usize,
    depth: u8,
    prefix: u64,
    child_idx: usize,
}

impl<'a> Iterator for RadixTreeIterator<'a> {
    type Item = u64;

    fn next(&mut self) -> Option<Self::Item> {
        while let Some(frame) = self.stack.pop() {
            let node = &self.tree.nodes[frame.node];

            if frame.depth == 5 {
                if node.terminal {
                    let val = Board::from_compressed_repr(frame.prefix).0;
                    return Some(val);
                }
                continue;
            }

            if frame.child_idx < node.children.len() {
                self.stack.push(Frame {
                    child_idx: frame.child_idx + 1,
                    ..frame
                });
                let (byte, child) = node.children[frame.child_idx];
                self.stack.push(Frame {
                    node: child as usize,
                    depth: frame.depth + 1,
                    prefix: (frame.prefix << 8) | byte as u64,
                    child_idx: 0,
                });
            }
        }
        None
    }
}

impl<'a> IntoIterator for &'a RadixTree {
    type Item = u64;

    type IntoIter = RadixTreeIterator<'a>;

    fn into_iter(self) -> Self::IntoIter {
        RadixTreeIterator {
            tree: self,
            stack: vec![Frame {
                node: 0,
                depth: 0,
                prefix: 0,
                child_idx: 0,
            }],
        }
    }
}

impl RadixTree {
    pub fn new() -> Self {
        Self {
            len: 0,
            nodes: vec![Node {
                children: vec![],
                terminal: false,
            }],
        }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    /// Get the child node index for a given byte
    fn get_child(&self, node: usize, byte: u8) -> Option<u32> {
        self.nodes[node]
            .children
            .iter()
            .find(|(b, _)| *b == byte)
            .map(|(_, idx)| *idx)
    }

    fn get_or_create_child(&mut self, node: usize, byte: u8) -> u32 {
        if let Some(idx) = self.get_child(node, byte) {
            return idx;
        }

        let new_idx = self.nodes.len() as u32;
        self.nodes.push(Node {
            children: vec![],
            terminal: false,
        });

        self.nodes[node].children.push((byte, new_idx));
        new_idx
    }

    pub fn insert(&mut self, value: u64) -> bool {
        let value = Board(value).to_compressed_repr();
        let mut node = 0;

        for shift in (0..40).step_by(8).rev() {
            let byte = ((value >> shift) & 0xff) as u8;
            node = self.get_or_create_child(node, byte) as usize;
        }

        let was_new = !self.nodes[node].terminal;
        self.nodes[node].terminal = true;
        if was_new {
            self.len += 1;
        }
        was_new
    }

    pub fn contains(&self, value: u64) -> bool {
        let value = Board(value).to_compressed_repr();
        let mut node = 0;

        for shift in (0..40).step_by(8).rev() {
            let byte = ((value >> shift) & 0xff) as u8;
            match self.get_child(node, byte) {
                Some(next) => node = next as usize,
                None => return false,
            }
        }
        self.nodes[node].terminal
    }
}
