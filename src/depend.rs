use crate::error::{ErrorClass, MonorailError};
use std::cmp::{Eq, PartialEq};
use std::collections::{HashMap, HashSet, VecDeque};

#[derive(Debug, Eq, PartialEq)]
enum CycleState {
    Unknown,
    Yes(usize),
    No,
}

// An adjacency list-backed DAG of targets.
#[derive(Debug)]
pub struct Dag {
    // Adjacency list storing dependencies.
    adj_list: Vec<Vec<usize>>,
    // Lazily-evaluated cycle detection state. Any algorithms that
    // can passively calculate cycle state will do so if this is
    // set to CycleState::Unknown.
    cycle_state: CycleState,

    pub label2node: HashMap<String, usize>,
    pub node2label: HashMap<usize, String>,
}

impl Dag {
    pub fn new(size: usize) -> Self {
        Self {
            adj_list: vec![vec![]; size],
            cycle_state: CycleState::Unknown,
            label2node: HashMap::with_capacity(size),
            node2label: HashMap::with_capacity(size),
        }
    }

    // Update the given node in the dag and its label. This is not bounds checked,
    // as our use of this graph has us always setting on a known graph size.
    // NOTE: do not call this after another function that calls `set_cycle_state`,
    // as this could make that value incorrect.
    pub fn set(&mut self, node: usize, nodes: Vec<usize>) {
        self.adj_list[node] = nodes;
    }

    pub fn set_label(&mut self, label: String, node: usize) {
        // update internal hashmaps
        self.label2node.insert(label.to_owned(), node);
        self.node2label.insert(node, label);
    }

    // Uses Kahn's Algorithm to walk the graph and build lists of nodes that
    // are independent of each other at that level. In addition, this function
    // will compute a cycle detection if it is not yet known.
    pub fn get_groups(&mut self) -> Result<Vec<Vec<usize>>, MonorailError> {
        self.check_acyclic()?;

        let mut groups = Vec::new();
        let mut in_degree = vec![0; self.adj_list.len()];
        let mut work = VecDeque::new();

        // calculate in-degree for every node; i.e. the number of nodes that
        // point to each node
        for nodes in self.adj_list.iter() {
            for &n in nodes {
                in_degree[n] += 1;
            }
        }
        // queue any nodes with 0 in edges for first processing group
        for (node, &degree) in in_degree.iter().enumerate() {
            if degree == 0 {
                work.push_back(node);
            }
        }
        // start the processing
        while !work.is_empty() {
            let mut current_group = vec![];
            let mut next_work = VecDeque::new();

            while let Some(n1) = work.pop_front() {
                current_group.push(n1);
                for &n2 in &self.adj_list[n1] {
                    in_degree[n2] -= 1;
                    if in_degree[n2] == 0 {
                        next_work.push_front(n2);
                    }
                }
            }

            groups.push(current_group);
            work = next_work;
        }

        // set cycle state if it is not yet known, as we have already
        // calculated in degrees here and can do so efficiently
        if self.cycle_state == CycleState::Unknown {
            self.set_cycle_state(in_degree.as_slice());
            self.check_acyclic()?;
        }

        Ok(groups)
    }

    fn check_acyclic(&self) -> Result<(), MonorailError> {
        if let CycleState::Yes(cycle_node) = self.cycle_state {
            let label = self
                .node2label
                .get(&cycle_node)
                .ok_or_else(|| MonorailError {
                    class: ErrorClass::DependencyGraph,
                    message: format!("Node {} label not found", &cycle_node),
                })?;
            return Err(MonorailError {
                class: ErrorClass::DependencyGraph,
                message: format!(
                    "Cycle detected at node: '{}' ({}), aborting",
                    label, cycle_node
                ),
            });
        }
        Ok(())
    }

    fn set_cycle_state(&mut self, in_degree: &[usize]) {
        for (i, &node) in in_degree.iter().enumerate() {
            if node != 0 {
                self.cycle_state = CycleState::Yes(i);
                return;
            }
        }
        self.cycle_state = CycleState::No;
    }
}

mod tests {
    use super::*;

    #[test]
    fn test_dag_set() {
        let mut dag = Dag::new(3);
        dag.set(0, vec![1]);
        assert_eq!(dag.adj_list[0], &[1]);
        dag.set(2, vec![]);
        assert_eq!(dag.adj_list[2], Vec::<usize>::new().as_slice());
    }

    #[test]
    fn test_dag_get_groups() {
        let mut dag = Dag::new(4);

        dag.set(0, vec![1, 2]);
        dag.set(1, vec![2]);
        dag.set(2, vec![]);
        dag.set(3, vec![1]);

        let g = dag.get_groups().unwrap();
        assert_eq!(g[0], &[0, 3]);
        assert_eq!(g[1], &[1]);
        assert_eq!(g[2], &[2]);
    }

    #[test]
    fn test_dag_get_groups_err_cyclic() {
        let mut dag = Dag::new(4);
        dag.set(0, vec![1]);
        dag.set(1, vec![0]);

        assert!(dag.get_groups().is_err());
    }
}
