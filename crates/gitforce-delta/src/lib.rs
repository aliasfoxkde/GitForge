//! GitForge Delta Executor
//!
//! Analyzes git diffs to determine which packages, workflows, and tests
//! are affected by a change — enabling precise CI/CD execution instead
//! of running everything.
//!
//! # How It Works
//!
//! 1. **Diff Analysis** — Parse git diff to get list of changed files
//! 2. **Package Mapping** — Map changed files to their owning packages
//! 3. **Dependency Graph** — Walk reverse dependencies (if X changed, Y may break)
//! 4. **Scope Resolution** — Determine what to run: lint-only, test, build, etc.
//!
//! # Example (not run)
//!
//! ```no_run
//! # use gitforce_delta::DeltaAnalyzer;
//! # #[tokio::main]
//! # async fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let analyzer = DeltaAnalyzer::new(".", "HEAD~1");
//! let plan = analyzer.analyze().await?;
//! println!("Affected packages: {:?}", plan.affected_packages);
//! println!("Run: {:?}", plan.execution_scope);
//! # Ok(())
//! # }
//! ```

pub mod analyzer;
pub mod mapper;

pub use analyzer::{ChangeType, DeltaAnalyzer, DeltaPlan, ExecutionScope};
pub use mapper::{PackageMapper, PackageMapping, ProjectType};
