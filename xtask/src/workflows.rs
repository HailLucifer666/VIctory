//! `cargo xtask workflows`: every file in `.github/workflows/` must be one
//! GitHub can load.
//!
//! GitHub reads the workflow files before it creates a job, so a file it
//! cannot read produces no run at all. Not a failing check — an absent one.
//! On 2026-11-26 `ci.yml` carried a step whose `-` sat two columns left of its
//! siblings: every other entry in the list started at column seven, that one
//! at five, and a block sequence is delimited only by its entries sharing a
//! column. There was no reading of the file, so the parser refused it — and
//! the edit was being made to the check that would have caught it.
//!
//! A parse alone would not have been enough. A file can be valid YAML and
//! still describe no job: a `steps:` block at the wrong depth parses cleanly
//! and runs nothing. So the shape is asserted too.

use std::path::Path;

use anyhow::{Context, Result, bail};
use yaml_rust2::yaml::Hash;
use yaml_rust2::{Yaml, YamlLoader};

/// Read one workflow file and return every reason GitHub would refuse it.
///
/// Returned rather than printed so the checks can be tested against a document
/// directly, with no filesystem in the way.
pub fn problems(file: &str, source: &str) -> Vec<String> {
    let mut found = Vec::new();

    let docs = match YamlLoader::load_from_str(source) {
        Ok(docs) => docs,
        Err(e) => {
            found.push(format!("{file}: not valid YAML: {e}"));
            return found;
        }
    };

    let doc = match docs.as_slice() {
        [doc] => doc,
        [] => {
            found.push(format!("{file}: empty; a workflow is one document"));
            return found;
        }
        _ => {
            found.push(format!(
                "{file}: {} documents; a workflow file is exactly one",
                docs.len()
            ));
            return found;
        }
    };

    let Yaml::Hash(top) = doc else {
        found.push(format!("{file}: the document is not a mapping"));
        return found;
    };

    // `on` is the trigger list. YAML 1.2 keeps it a string; a loader resolving
    // it under YAML 1.1 turns it into the boolean `true`. Accept both rather
    // than pin the check to the loader's version.
    let has_trigger =
        top.contains_key(&Yaml::String("on".to_owned())) || top.contains_key(&Yaml::Boolean(true));
    if !has_trigger {
        found.push(format!(
            "{file}: no `on:`; GitHub refuses a workflow with no trigger"
        ));
    }

    let jobs = match field(top, "jobs") {
        Some(Yaml::Hash(jobs)) if !jobs.is_empty() => jobs,
        Some(Yaml::Hash(_)) => {
            found.push(format!("{file}: `jobs` is empty"));
            return found;
        }
        Some(_) => {
            found.push(format!("{file}: `jobs` is not a mapping"));
            return found;
        }
        None => {
            found.push(format!("{file}: no `jobs:`"));
            return found;
        }
    };

    for (key, job) in jobs {
        let name = match key {
            Yaml::String(s) => s.clone(),
            other => format!("{other:?}"),
        };
        let Yaml::Hash(job) = job else {
            found.push(format!("{file}: job `{name}` is not a mapping"));
            continue;
        };
        // A job is either a set of steps or a call into another workflow.
        if matches!(field(job, "uses"), Some(Yaml::String(_))) {
            continue;
        }
        match field(job, "steps") {
            Some(Yaml::Array(steps)) if !steps.is_empty() => {
                for (i, step) in steps.iter().enumerate() {
                    let Yaml::Hash(step) = step else {
                        found.push(format!(
                            "{file}: job `{name}` step {} is not a mapping",
                            i + 1
                        ));
                        continue;
                    };
                    if field(step, "uses").is_none() && field(step, "run").is_none() {
                        found.push(format!(
                            "{file}: job `{name}` step {} has neither `uses` nor `run`",
                            i + 1
                        ));
                    }
                }
            }
            Some(Yaml::Array(_)) => {
                found.push(format!("{file}: job `{name}` has no steps"));
            }
            Some(_) => {
                found.push(format!("{file}: job `{name}`'s `steps` is not a list"));
            }
            None => {
                found.push(format!(
                    "{file}: job `{name}` has neither `steps` nor `uses`"
                ));
            }
        }
    }

    found
}

/// Every problem across every workflow file in the tree, one line each.
pub fn scan(root: &Path) -> Result<Vec<String>> {
    let dir = root.join(".github").join("workflows");
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e).with_context(|| format!("reading {}", dir.display())),
    };

    let mut found = Vec::new();
    for entry in entries {
        let path = entry
            .with_context(|| format!("reading {}", dir.display()))?
            .path();
        if !path.is_file() {
            continue;
        }
        // GitHub reads the top level of the directory only, so subdirectories
        // are not workflows and are not checked.
        let is_workflow = matches!(
            path.extension().and_then(|e| e.to_str()),
            Some("yml" | "yaml")
        );
        if !is_workflow {
            continue;
        }
        let source = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("<workflow>");
        found.extend(problems(name, &source));
    }
    found.sort();
    Ok(found)
}

pub fn run(root: &Path) -> Result<()> {
    let found = scan(root)?;
    if found.is_empty() {
        println!("workflows: every file under .github/workflows/ is one GitHub can load");
        return Ok(());
    }
    for line in &found {
        eprintln!("{line}");
    }
    bail!(
        "{} workflow problem(s). GitHub reads these files before it creates a job, so a file it cannot load produces no run at all",
        found.len()
    );
}

fn field<'a>(map: &'a Hash, name: &str) -> Option<&'a Yaml> {
    map.get(&Yaml::String(name.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The `steps:` block as it stood in `ci.yml` on 2026-11-26, before the
    /// fix: the new step's `-` at column five and its `run:` at six, while
    /// every sibling entry starts at column seven. Kept verbatim, so the guard
    /// is proved against the shape that actually shipped rather than one
    /// written to suit it.
    const STEP_INDENTED_SHORT: &str = "\
name: CI
on:
  pull_request:
jobs:
  check:
    runs-on: windows-latest
    steps:
      - uses: actions/checkout@v5
      # Runs tests and the gate in one pass.
    # and fails on a run that executed nothing.
    - name: Tests, gate
      run: cargo xtask test-gate --workspace --locked
";

    /// The same file with every step, and every key inside a step, on its
    /// siblings' column. This is the negative control: the check must accept
    /// it, or the red above proves only that the check refuses everything.
    const LOADABLE: &str = "\
name: CI
on:
  pull_request:
jobs:
  check:
    runs-on: windows-latest
    steps:
      - uses: actions/checkout@v5
      # Runs tests and the gate in one pass.
      # and fails on a run that executed nothing.
      - name: Tests, gate
        run: cargo xtask test-gate --workspace --locked
";

    #[test]
    fn a_step_indented_short_of_its_siblings_is_refused() {
        let found = problems("ci.yml", STEP_INDENTED_SHORT);
        assert!(
            found.iter().any(|p| p.contains("not valid YAML")),
            "the 2026-11-26 ci.yml was accepted: {found:#?}"
        );
    }

    #[test]
    fn a_loadable_workflow_is_accepted() {
        assert_eq!(problems("ci.yml", LOADABLE), Vec::<String>::new());
    }

    /// Valid YAML, and no job in it: `steps` sits beside `jobs` instead of
    /// inside one. This is the case a parse alone cannot see.
    #[test]
    fn steps_outside_a_job_are_refused() {
        let source = "\
name: CI
on: [pull_request]
jobs:
  check:
    runs-on: windows-latest
steps:
  - uses: actions/checkout@v5
";
        let found = problems("ci.yml", source);
        assert!(
            found
                .iter()
                .any(|p| p.contains("job `check` has neither `steps` nor `uses`")),
            "{found:#?}"
        );
    }

    #[test]
    fn a_workflow_with_no_trigger_is_refused() {
        let source = "\
name: CI
jobs:
  check:
    runs-on: windows-latest
    steps:
      - uses: actions/checkout@v5
";
        let found = problems("ci.yml", source);
        assert!(found.iter().any(|p| p.contains("no `on:`")), "{found:#?}");
    }

    #[test]
    fn a_step_with_neither_uses_nor_run_is_refused() {
        let source = "\
name: CI
on: [pull_request]
jobs:
  check:
    runs-on: windows-latest
    steps:
      - name: nothing happens
";
        let found = problems("ci.yml", source);
        assert!(
            found.iter().any(|p| p.contains("neither `uses` nor `run`")),
            "{found:#?}"
        );
    }

    /// A job that calls another workflow has no steps and is still valid.
    #[test]
    fn a_job_that_calls_another_workflow_is_accepted() {
        let source = "\
name: CI
on: [pull_request]
jobs:
  call:
    uses: ./.github/workflows/other.yml
";
        assert_eq!(problems("ci.yml", source), Vec::<String>::new());
    }

    /// The guard that matters: the real tree. It walks the directory rather
    /// than naming `ci.yml`, so the next workflow file is covered without
    /// anyone remembering to add it.
    #[test]
    fn every_workflow_in_the_repository_loads() {
        let found = scan(&crate::workspace_root()).expect("reading .github/workflows");
        assert!(found.is_empty(), "{}", found.join("\n"));
    }
}
