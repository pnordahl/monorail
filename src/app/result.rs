use std::path;
use std::result::Result;

use crate::app::run;
use crate::core::{self, error::MonorailError, tracking};

pub(crate) const RESULT_OUTPUT_FILE_NAME: &str = "result.json.zst";

#[derive(Debug)]
pub(crate) struct ResultShowInput {}

pub(crate) fn result_show<'a>(
    cfg: &'a core::Config,
    work_path: &'a path::Path,
    _input: &'a ResultShowInput,
) -> Result<run::RunOutput, MonorailError> {
    // open tracking and get run
    let tracking_table = tracking::Table::new(&cfg.get_tracking_path(work_path))?;
    // use run to get results.json file in id dir
    let run = tracking_table.open_run()?;
    let run_dir = cfg.get_run_path(work_path).join(format!("{}", run.id));
    let run_output_file = std::fs::OpenOptions::new()
        .read(true)
        .open(run_dir.join(RESULT_OUTPUT_FILE_NAME))
        .map_err(|e| MonorailError::Generic(e.to_string()))?;
    let br = std::io::BufReader::new(run_output_file);
    let mut decoder = zstd::stream::read::Decoder::new(br)?;

    Ok(serde_json::from_reader(&mut decoder)?)
}
