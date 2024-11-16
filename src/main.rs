mod slice_trim_ext;
mod tests;

use std::{num::ParseIntError, sync::Arc, time::Duration};

use chrono::Local;
use clap::Parser;
use color_eyre::Result;
use env_logger::fmt::style::{AnsiColor, Style};
use futures::stream::{FuturesUnordered, StreamExt};
use indicatif::{MultiProgress, ProgressBar};
use indicatif_log_bridge::LogWrapper;
use log::{error, info};
use std::io::Write;
use tests::{get_tests, TestTimeoutResult};
use tokio::sync::Semaphore;

#[derive(Parser, Debug, Clone)]
#[command(version, about, long_about = None)]
struct Args {
    /// The name of the task to test
    task: String,

    /// The command to run (defaults to the task name, with .exe on Windows)
    #[arg(short, long)]
    command: Option<String>,

    /// Input filename pattern
    #[arg(short, long, default_value = "in/{task}{test}.in")]
    in_pattern: String,

    /// Output filename patern
    #[arg(short, long, default_value = "out/{task}{test}.out")]
    out_pattern: String,

    /// Timeout for program execution
    #[arg(short, long, value_parser = parse_duration, default_value = "5")]
    timeout: Duration,

    /// How many tests can be ran in parallel
    #[arg(short, long, default_value_t = 5)]
    parallel: usize,
}

fn parse_duration(arg: &str) -> Result<Duration, ParseIntError> {
    Ok(Duration::from_secs(arg.parse()?))
}

#[derive(Debug, Clone)]
struct TestStats {
    pub pass: Vec<String>,
    pub fail: Vec<String>,
    pub timeout: Vec<String>,
}

impl TestStats {
    pub fn new() -> Self {
        Self {
            pass: vec![],
            fail: vec![],
            timeout: vec![],
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    color_eyre::install()?;

    let logger = env_logger::builder()
        .filter_level(log::LevelFilter::Info)
        .format(|buf, record| {
            let subtle = Style::new().fg_color(Some(AnsiColor::BrightBlack.into()));
            let level_style = buf.default_level_style(record.level());

            writeln!(
                buf,
                "{subtle}[{subtle:#}{} {level_style}{:<5}{level_style:#}{subtle}]{subtle:#} {}",
                Local::now().format("%d-%m-%Y %H:%M:%S"),
                record.level(),
                record.args()
            )
        })
        .build();

    let args = Args::parse();

    let tests = get_tests(&args)?;

    let multi = MultiProgress::new();
    LogWrapper::new(multi.clone(), logger).try_init()?;
    let progress_bar = multi.add(ProgressBar::new(tests.len().try_into()?));

    let progress_bar = Arc::new(progress_bar);

    let semaphore = Arc::new(Semaphore::new(args.parallel));

    info!(
        "Loaded {} tests for task {}. Running {} tests in parallel.",
        tests.len(),
        &args.task,
        &args.parallel
    );

    let tests: FuturesUnordered<_> = tests
        .into_iter()
        .map(|test| {
            let progress_bar = progress_bar.clone();
            let semaphore = semaphore.clone();

            let args = args.clone();

            tokio::spawn(async move {
                let _permit = semaphore.acquire().await.unwrap();

                let name = test.name.clone();
                let ret = test.run(&args).await;
                if let Err(e) = &ret {
                    error!("✖ Test {} - ERROR\n{:?}", name, e);
                }
                progress_bar.inc(1);
                ret
            })
        })
        .collect();

    let results: Vec<_> = tests
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .filter_map(|x| x.ok())
        .filter_map(|x| x.ok())
        .collect();

    let mut stats = TestStats::new();

    for test in results.iter() {
        match test {
            TestTimeoutResult::TimedOut(name) => {
                stats.timeout.push(name.to_string());
            }
            TestTimeoutResult::Finished(res) => {
                if res.correct {
                    stats.pass.push(res.name.clone());
                } else {
                    stats.fail.push(res.name.clone());
                }
            }
        }
    }

    info!(
        "*** TEST REPORT ***\n✔ PASS: {}\n✖ FAIL: {}\n✖ TIMEOUT: {}",
        stats.pass.len(),
        stats.fail.len(),
        stats.timeout.len()
    );

    progress_bar.finish();

    Ok(())
}
