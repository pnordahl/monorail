use crate::core::{self, error::MonorailError};

use std::io::{self};
use std::{fs, path};

use serde::Serialize;
use sha2::Digest;
use tracing::info;

#[derive(Debug, Serialize)]
pub(crate) struct ConfigGenerateInput<'a> {
    pub(crate) output_file: &'a str,
}
#[derive(Debug, Serialize)]
pub(crate) struct ConfigGenerateOutput {
    pub(crate) config: core::Config,
}

pub(crate) fn config_generate(
    input: ConfigGenerateInput,
    work_path: &path::Path,
) -> Result<ConfigGenerateOutput, MonorailError> {
    info!(output_file = &input.output_file, "Generated path");
    // use the target output file's parent to derive the monorail-out
    // tracking dir that we should write the output file checksum to
    let output_file_parent =
        path::Path::new(input.output_file)
            .parent()
            .ok_or(MonorailError::Generic(format!(
                "Generated config file {} has no parent directory",
                input.output_file
            )))?;

    let stdin = io::stdin();
    let handle = stdin.lock();
    let mut hasher = sha2::Sha256::new();

    // read the source data and update it with the source file's checksum
    let mut config: core::Config = serde_json::from_reader(handle)?;
    match config.source {
        Some(ref mut source) => {
            hasher.update(fs::read(work_path.join(&source.path))?);
            source.algorithm = Some(core::AlgorithmKind::Sha256);
            source.checksum = Some(format!("{:x}", hasher.finalize_reset()));
            info!(checksum = &source.checksum, "Source checksum");
        }
        None => {
            return Err(MonorailError::from(
                "Source data must populate 'source.path' with a relative path to the input file",
            ));
        }
    }

    // serialize the modified config and checksum it
    let config_data = serde_json::to_vec_pretty(&config)?;
    hasher.update(&config_data);
    let checksum = format!("{:x}", hasher.finalize());
    info!(checksum = &checksum, "Generated checksum");

    // write the checksum of the final config to the tracking table
    // for that config, ensuring that it can be used for comparison
    // when this new config is used in other commands
    let config_tracking_path = config.get_tracking_path(output_file_parent);
    let tt = core::tracking::Table::new(&config_tracking_path)?;
    info!(
        tracking_path = &config_tracking_path.display().to_string(),
        "Generated tracking path"
    );
    // fs::create_dir_all(&config_tracking_path)?;
    let checksum_tracking_path = config_tracking_path.join("generated_config_checksum");
    info!(
        path = &checksum_tracking_path.display().to_string(),
        "Generated checksum tracking path"
    );
    // fs::write(tt.get_generated_checksum_path(), checksum)?;
    tt.save_generated_config_checksum(checksum)?;
    fs::write(input.output_file, config_data)?;
    info!("Generated config and checksum written");

    Ok(ConfigGenerateOutput { config })
}
