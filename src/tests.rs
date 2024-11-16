use std::{
    path::PathBuf,
    process::{Output, Stdio},
    time::{Duration, Instant},
};

use color_eyre::{eyre::ContextCompat, Report, Result};
use glob::glob;
use itertools::Itertools;
use log::{debug, error, info};
use tokio::{fs, io::AsyncWriteExt, process::Command, time::timeout};

use crate::slice_trim_ext::SliceTrimExt;

#[derive(Debug, Clone)]
pub struct Test {
    pub name: String,

    in_file: PathBuf,
    out_file: PathBuf,
}

impl Test {
    pub async fn run(self, args: &crate::Args) -> Result<TestTimeoutResult> {
        let command = args.command.clone().unwrap_or_else(|| {
            if cfg!(windows) {
                format!("{}.exe", args.task.clone())
            } else {
                args.task.clone()
            }
        });

        debug!("Running test {}...", &self.name);
        let start_time = Instant::now();

        let res = timeout(args.timeout, async {
            let mut child = Command::new(command)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .kill_on_drop(true)
                .spawn()?;

            {
                let mut stdin = child.stdin.take().context("Failed to take child's stdin")?;

                stdin.write(&self.get_input().await?).await?;
            }

            Ok::<_, Report>(child.wait_with_output().await?)
        })
        .await;

        let elapsed = start_time.elapsed();

        Ok(match res {
            Ok(output) => {
                let output = output?;
                let correct = self.is_correct(output.stdout.clone()).await?;

                if correct {
                    info!(
                        "✔ Test {} - PASS ({:.2} s)",
                        &self.name,
                        &elapsed.as_secs_f64()
                    );
                } else {
                    error!(
                        "✖ Test {} - FAIL ({:.2} s)\nExpected: {}\nGot: {}",
                        &self.name,
                        &elapsed.as_secs_f64(),
                        String::from_utf8(self.get_output().await?.as_slice().trim().to_vec())?,
                        String::from_utf8(output.stdout.clone().as_slice().trim().to_vec())?,
                    );
                }

                TestTimeoutResult::Finished(TestResult {
                    name: self.name.clone(),

                    time: elapsed,
                    correct,

                    stdin: self.get_input().await?,
                    output,
                })
            }
            Err(_) => {
                error!("✖ Test {} - TIMED OUT!", &self.name);
                TestTimeoutResult::TimedOut(self.name)
            }
        })
    }

    async fn get_input(&self) -> Result<Vec<u8>> {
        Ok(fs::read(&self.in_file).await?)
    }

    async fn get_output(&self) -> Result<Vec<u8>> {
        Ok(fs::read(&self.out_file).await?)
    }

    async fn is_correct(&self, actual: Vec<u8>) -> Result<bool> {
        let expected = self.get_output().await?;

        let actual = actual.as_slice().trim();
        let expected = expected.as_slice().trim();

        Ok(actual == expected)
    }
}

#[derive(Debug, Clone)]
pub enum TestTimeoutResult {
    TimedOut(
        /// The name of the test
        String,
    ),

    Finished(TestResult),
}

#[derive(Debug, Clone)]
pub struct TestResult {
    pub name: String,

    pub time: Duration,
    pub correct: bool,

    pub stdin: Vec<u8>,
    pub output: Output,
}

pub fn get_tests(args: &crate::Args) -> Result<Vec<Test>> {
    let task = args.task.clone();
    let task_in_pattern = args.in_pattern.replace("{task}", &task);

    Ok(glob(&task_in_pattern.replace("{test}", "*"))?
        .map_ok(|x| -> Result<Test> {
            let path_str = x.to_string_lossy();

            let test_pos = task_in_pattern
                .find("{test}")
                .context("{test} not found in in_pattern")?;

            let test_name = path_str[test_pos
                ..(path_str.len() - (task_in_pattern.len() - (test_pos + "{test}".len())))]
                .to_string();

            Ok(Test {
                name: test_name.clone(),
                in_file: x,
                out_file: PathBuf::from(
                    args.out_pattern
                        .replace("{task}", &task)
                        .replace("{test}", &test_name),
                ),
            })
        })
        .flatten()
        .collect::<Result<Vec<Test>>>()?)
}
