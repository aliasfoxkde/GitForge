//! DAG (Directed Acyclic Graph) builder for job dependencies

use crate::pipeline::{JobDefinition, PipelineDefinition};
use gitforge_common::{JobId, PipelineRunId, Result};
use std::collections::{HashMap, HashSet};

/// DAG node representing a job
#[derive(Debug, Clone)]
pub struct JobNode {
    pub id: JobId,
    pub name: String,
    pub definition: JobDefinition,
    pub dependencies: Vec<JobId>,
}

/// Job graph representing the pipeline DAG
#[derive(Debug, Clone)]
pub struct JobGraph {
    pub nodes: Vec<JobNode>,
    pub run_id: PipelineRunId,
}

impl JobGraph {
    /// Get nodes with no dependencies (entry points)
    pub fn entry_points(&self) -> Vec<&JobNode> {
        self.nodes
            .iter()
            .filter(|n| n.dependencies.is_empty())
            .collect()
    }

    /// Get a node by ID
    pub fn get(&self, id: JobId) -> Option<&JobNode> {
        self.nodes.iter().find(|n| n.id == id)
    }

    /// Get a node by name
    pub fn get_by_name(&self, name: &str) -> Option<&JobNode> {
        self.nodes.iter().find(|n| n.name == name)
    }

    /// Get nodes that depend on the given node
    pub fn dependents(&self, id: JobId) -> Vec<&JobNode> {
        self.nodes
            .iter()
            .filter(|n| n.dependencies.contains(&id))
            .collect()
    }

    /// Check if the graph has a cycle (invalid DAG)
    pub fn has_cycle(&self) -> bool {
        let mut visited = HashSet::new();
        let mut recursion_stack = HashSet::new();

        for node in &self.nodes {
            if self.detect_cycle_dfs(node.id, &mut visited, &mut recursion_stack) {
                return true;
            }
        }
        false
    }

    fn detect_cycle_dfs(
        &self,
        node_id: JobId,
        visited: &mut HashSet<JobId>,
        recursion_stack: &mut HashSet<JobId>,
    ) -> bool {
        if recursion_stack.contains(&node_id) {
            return true;
        }
        if visited.contains(&node_id) {
            return false;
        }

        visited.insert(node_id);
        recursion_stack.insert(node_id);

        if let Some(node) = self.get(node_id) {
            for dep in &node.dependencies {
                if self.detect_cycle_dfs(*dep, visited, recursion_stack) {
                    return true;
                }
            }
        }

        recursion_stack.remove(&node_id);
        false
    }

    /// Get topological order of nodes
    pub fn topological_order(&self) -> Vec<JobId> {
        let mut result = Vec::new();
        let mut visited = HashSet::new();

        for node in &self.nodes {
            self.visit(node.id, &mut visited, &mut result);
        }

        result
    }

    fn visit(&self, node_id: JobId, visited: &mut HashSet<JobId>, result: &mut Vec<JobId>) {
        if visited.contains(&node_id) {
            return;
        }

        visited.insert(node_id);

        // Visit dependencies first
        if let Some(node) = self.get(node_id) {
            for dep in &node.dependencies {
                self.visit(*dep, visited, result);
            }
        }

        result.push(node_id);
    }
}

/// DAG builder for pipeline jobs
pub struct DagBuilder;

impl DagBuilder {
    /// Build a job graph from a pipeline definition
    pub fn build(pipeline: &PipelineDefinition, run_id: PipelineRunId) -> Result<JobGraph> {
        let mut nodes = Vec::new();
        let mut name_to_id = HashMap::new();

        // First pass: create all nodes
        for job in &pipeline.jobs {
            let job_id = JobId::new();
            if name_to_id.insert(job.name.clone(), job_id).is_some() {
                return Err(gitforge_common::Error::invalid_input(format!(
                    "pipeline contains duplicate job name '{}'",
                    job.name
                )));
            }

            nodes.push(JobNode {
                id: job_id,
                name: job.name.clone(),
                definition: job.clone(),
                dependencies: Vec::new(),
            });
        }

        // Second pass: resolve dependencies
        for node in &mut nodes {
            let mut dep_ids = Vec::with_capacity(node.definition.needs.len());
            for name in &node.definition.needs {
                let Some(dep_id) = name_to_id.get(name).copied() else {
                    return Err(gitforge_common::Error::invalid_input(format!(
                        "job '{}' references missing dependency '{}'",
                        node.name, name
                    )));
                };
                dep_ids.push(dep_id);
            }

            node.dependencies = dep_ids;
        }

        let graph = JobGraph { nodes, run_id };

        // Validate no cycles
        if graph.has_cycle() {
            return Err(gitforge_common::Error::invalid_input(
                "pipeline contains circular dependencies",
            ));
        }

        Ok(graph)
    }

    /// Get the maximum depth of any node in the graph
    pub fn max_depth(graph: &JobGraph) -> usize {
        let mut depths: HashMap<JobId, usize> = HashMap::new();

        for node in &graph.nodes {
            let depth = Self::compute_depth(graph, node.id, &mut depths);
            depths.insert(node.id, depth);
        }

        depths.values().max().copied().unwrap_or(0)
    }

    fn compute_depth(
        graph: &JobGraph,
        node_id: JobId,
        depths: &mut HashMap<JobId, usize>,
    ) -> usize {
        if let Some(&depth) = depths.get(&node_id) {
            return depth;
        }

        let node = match graph.get(node_id) {
            Some(n) => n,
            None => return 0,
        };

        if node.dependencies.is_empty() {
            return 0;
        }

        let max_dep_depth = node
            .dependencies
            .iter()
            .map(|dep| Self::compute_depth(graph, *dep, depths))
            .max()
            .unwrap_or(0);

        max_dep_depth + 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::StepDefinition;
    use std::collections::HashMap;

    fn make_job(name: &str, needs: Vec<&str>) -> JobDefinition {
        JobDefinition {
            name: name.to_string(),
            image: "rust:latest".to_string(),
            needs: needs.into_iter().map(|s| s.to_string()).collect(),
            env: HashMap::new(),
            steps: vec![StepDefinition {
                name: "step1".to_string(),
                run: "echo hello".to_string(),
                env: None,
                working_directory: None,
                condition: None,
            }],
            timeout: None,
            retry: None,
        }
    }

    #[test]
    fn test_build_simple_dag() {
        let pipeline = PipelineDefinition {
            name: "test".to_string(),
            version: "1.0".to_string(),
            trigger_on: vec![],
            environment: HashMap::new(),
            jobs: vec![make_job("build", vec![]), make_job("test", vec!["build"])],
        };

        let run_id = gitforge_common::PipelineRunId::new();
        let graph = DagBuilder::build(&pipeline, run_id).unwrap();

        assert_eq!(graph.nodes.len(), 2);
        assert!(!graph.has_cycle());

        let build = graph.get_by_name("build").unwrap();
        assert!(build.dependencies.is_empty());

        let test = graph.get_by_name("test").unwrap();
        assert_eq!(test.dependencies.len(), 1);
    }

    #[test]
    fn test_detect_cycle() {
        let jobs = vec![
            make_job("a", vec!["c"]),
            make_job("b", vec!["a"]),
            make_job("c", vec!["b"]), // Creates cycle: a -> c -> b -> a
        ];

        let pipeline = PipelineDefinition {
            name: "test".to_string(),
            version: "1.0".to_string(),
            trigger_on: vec![],
            environment: HashMap::new(),
            jobs,
        };

        let run_id = gitforge_common::PipelineRunId::new();
        let result = DagBuilder::build(&pipeline, run_id);
        // Pipeline with cycle should return error
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.message.contains("circular"));
    }

    #[test]
    fn test_reject_missing_dependency() {
        let pipeline = PipelineDefinition {
            name: "test".to_string(),
            version: "1.0".to_string(),
            trigger_on: vec![],
            environment: HashMap::new(),
            jobs: vec![make_job("test", vec!["build"])],
        };

        let result = DagBuilder::build(&pipeline, gitforge_common::PipelineRunId::new());
        let err = result.unwrap_err();
        assert!(err.message.contains("missing dependency 'build'"));
    }

    #[test]
    fn test_reject_duplicate_job_name() {
        let pipeline = PipelineDefinition {
            name: "test".to_string(),
            version: "1.0".to_string(),
            trigger_on: vec![],
            environment: HashMap::new(),
            jobs: vec![make_job("build", vec![]), make_job("build", vec![])],
        };

        let result = DagBuilder::build(&pipeline, gitforge_common::PipelineRunId::new());
        let err = result.unwrap_err();
        assert!(err.message.contains("duplicate job name 'build'"));
    }

    #[test]
    fn test_topological_order() {
        let pipeline = PipelineDefinition {
            name: "test".to_string(),
            version: "1.0".to_string(),
            trigger_on: vec![],
            environment: HashMap::new(),
            jobs: vec![
                make_job("build", vec![]),
                make_job("test", vec!["build"]),
                make_job("deploy", vec!["test"]),
            ],
        };

        let run_id = gitforge_common::PipelineRunId::new();
        let graph = DagBuilder::build(&pipeline, run_id).unwrap();

        let order = graph.topological_order();
        assert_eq!(order.len(), 3);

        // build should come before test, test should come before deploy
        let build_idx = order
            .iter()
            .position(|id| graph.get(*id).unwrap().name == "build")
            .unwrap();
        let test_idx = order
            .iter()
            .position(|id| graph.get(*id).unwrap().name == "test")
            .unwrap();
        let deploy_idx = order
            .iter()
            .position(|id| graph.get(*id).unwrap().name == "deploy")
            .unwrap();

        assert!(build_idx < test_idx);
        assert!(test_idx < deploy_idx);
    }

    #[test]
    fn test_max_depth_single_job() {
        let pipeline = PipelineDefinition {
            name: "test".to_string(),
            version: "1.0".to_string(),
            trigger_on: vec![],
            environment: HashMap::new(),
            jobs: vec![make_job("build", vec![])],
        };

        let run_id = gitforge_common::PipelineRunId::new();
        let graph = DagBuilder::build(&pipeline, run_id).unwrap();
        assert_eq!(DagBuilder::max_depth(&graph), 0);
    }

    #[test]
    fn test_max_depth_nested() {
        let pipeline = PipelineDefinition {
            name: "test".to_string(),
            version: "1.0".to_string(),
            trigger_on: vec![],
            environment: HashMap::new(),
            jobs: vec![
                make_job("a", vec![]),
                make_job("b", vec!["a"]),
                make_job("c", vec!["b"]),
                make_job("d", vec!["c"]),
            ],
        };

        let run_id = gitforge_common::PipelineRunId::new();
        let graph = DagBuilder::build(&pipeline, run_id).unwrap();
        // Depth: a=0, b=1, c=2, d=3
        assert_eq!(DagBuilder::max_depth(&graph), 3);
    }

    #[test]
    fn test_get_by_name_found() {
        let pipeline = PipelineDefinition {
            name: "test".to_string(),
            version: "1.0".to_string(),
            trigger_on: vec![],
            environment: HashMap::new(),
            jobs: vec![make_job("build", vec![])],
        };

        let run_id = gitforge_common::PipelineRunId::new();
        let graph = DagBuilder::build(&pipeline, run_id).unwrap();
        assert!(graph.get_by_name("build").is_some());
    }

    #[test]
    fn test_get_by_name_not_found() {
        let pipeline = PipelineDefinition {
            name: "test".to_string(),
            version: "1.0".to_string(),
            trigger_on: vec![],
            environment: HashMap::new(),
            jobs: vec![make_job("build", vec![])],
        };

        let run_id = gitforge_common::PipelineRunId::new();
        let graph = DagBuilder::build(&pipeline, run_id).unwrap();
        assert!(graph.get_by_name("nonexistent").is_none());
    }

    #[test]
    fn test_dependents() {
        let pipeline = PipelineDefinition {
            name: "test".to_string(),
            version: "1.0".to_string(),
            trigger_on: vec![],
            environment: HashMap::new(),
            jobs: vec![
                make_job("build", vec![]),
                make_job("test", vec!["build"]),
                make_job("deploy", vec!["test"]),
            ],
        };

        let run_id = gitforge_common::PipelineRunId::new();
        let graph = DagBuilder::build(&pipeline, run_id).unwrap();

        // Find the build node
        let build_id = graph.get_by_name("build").unwrap().id;
        let deps = graph.dependents(build_id);
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "test");
    }

    #[test]
    fn test_dependents_none() {
        let pipeline = PipelineDefinition {
            name: "test".to_string(),
            version: "1.0".to_string(),
            trigger_on: vec![],
            environment: HashMap::new(),
            jobs: vec![make_job("build", vec![]), make_job("test", vec!["build"])],
        };

        let run_id = gitforge_common::PipelineRunId::new();
        let graph = DagBuilder::build(&pipeline, run_id).unwrap();

        // Find the deploy node (which doesn't exist as a dependency)
        let test_id = graph.get_by_name("test").unwrap().id;
        let deps = graph.dependents(test_id);
        assert!(deps.is_empty());
    }

    #[test]
    fn test_entry_points() {
        let pipeline = PipelineDefinition {
            name: "test".to_string(),
            version: "1.0".to_string(),
            trigger_on: vec![],
            environment: HashMap::new(),
            jobs: vec![make_job("build", vec![]), make_job("test", vec!["build"])],
        };

        let run_id = gitforge_common::PipelineRunId::new();
        let graph = DagBuilder::build(&pipeline, run_id).unwrap();

        let entry = graph.entry_points();
        assert_eq!(entry.len(), 1);
        assert_eq!(entry[0].name, "build");
    }

    #[test]
    fn test_multiple_entry_points() {
        let pipeline = PipelineDefinition {
            name: "test".to_string(),
            version: "1.0".to_string(),
            trigger_on: vec![],
            environment: HashMap::new(),
            jobs: vec![
                make_job("setup", vec![]),
                make_job("teardown", vec![]),
                make_job("test", vec!["setup", "teardown"]),
            ],
        };

        let run_id = gitforge_common::PipelineRunId::new();
        let graph = DagBuilder::build(&pipeline, run_id).unwrap();

        let entries = graph.entry_points();
        assert_eq!(entries.len(), 2);
        let names: Vec<_> = entries.iter().map(|n| n.name.as_str()).collect();
        assert!(names.contains(&"setup"));
        assert!(names.contains(&"teardown"));
    }

    #[test]
    fn test_get_node_by_id() {
        let pipeline = PipelineDefinition {
            name: "test".to_string(),
            version: "1.0".to_string(),
            trigger_on: vec![],
            environment: HashMap::new(),
            jobs: vec![make_job("build", vec![])],
        };

        let run_id = gitforge_common::PipelineRunId::new();
        let graph = DagBuilder::build(&pipeline, run_id).unwrap();

        let node = graph.get_by_name("build").unwrap();
        let found = graph.get(node.id);
        assert!(found.is_some());
        assert_eq!(found.unwrap().name, "build");
    }

    #[test]
    fn test_get_node_by_id_not_found() {
        let pipeline = PipelineDefinition {
            name: "test".to_string(),
            version: "1.0".to_string(),
            trigger_on: vec![],
            environment: HashMap::new(),
            jobs: vec![make_job("build", vec![])],
        };

        let run_id = gitforge_common::PipelineRunId::new();
        let graph = DagBuilder::build(&pipeline, run_id).unwrap();

        let fake_id = gitforge_common::JobId::new();
        assert!(graph.get(fake_id).is_none());
    }
}
