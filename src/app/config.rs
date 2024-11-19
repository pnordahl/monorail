use crate::core::{self, error::MonorailError};

use std::io::{self};
use std::{env, fs, path};

use serde::Serialize;
use sha2::Digest;
use tracing::info;

#[derive(Debug, Serialize)]
pub(crate) struct ConfigGenerateInput<'a> {
    pub(crate) output_file_path: &'a path::Path,
}
#[derive(Debug, Serialize)]
pub(crate) struct ConfigGenerateOutput {
    pub(crate) config: serde_json::Value,
}

pub(crate) fn config_generate(
    input: ConfigGenerateInput,
) -> Result<ConfigGenerateOutput, MonorailError> {
    info!(
        output_file = &input.output_file_path.display().to_string(),
        "Output path"
    );
    // use the target output file's parent to derive the directory
    // that we should write the output file and lockfile to
    let output_file_parent = input
        .output_file_path
        .parent()
        .ok_or(MonorailError::Generic(format!(
            "Output config file {} has no parent directory",
            input.output_file_path.display()
        )))?;
    fs::create_dir_all(output_file_parent)?;
    let lockfile_path = output_file_parent.join(format!(
        "{}.lock",
        core::file::get_stem(input.output_file_path)?
    ));
    info!(
        path = &lockfile_path.display().to_string(),
        "Output lockfile path"
    );

    // read the source into a value we can manipulate; we can't use Config
    // because it has defaults that, when serialized, will make viewing the
    // generated config for accuracy harder for the user (since it will
    // contain many fields they did not specify)
    let stdin = io::stdin();
    let handle = stdin.lock();
    let mut hasher = sha2::Sha256::new();
    let mut val: serde_json::Value = serde_json::from_reader(handle)?;
    let source_val = val.get_mut("source").ok_or(MonorailError::from(
        "Input data must populate 'source.path' with a relative path to the input file",
    ))?;
    let source = source_val
        .as_object_mut()
        .ok_or(MonorailError::from("Input data 'source' is not an object"))?;
    let source_path = source
        .get("path")
        .ok_or(MonorailError::from("Input data must provide 'source.path'"))?
        .as_str()
        .ok_or(MonorailError::from("Source path must be a string"))?;
    // source path is interpreted relative to the current directory
    hasher.update(fs::read(env::current_dir()?.join(source_path))?);
    source.insert(
        "algorithm".to_string(),
        serde_json::to_value(Some(core::AlgorithmKind::Sha256))?,
    );
    source.insert(
        "checksum".to_string(),
        serde_json::to_value(Some(format!("{:x}", hasher.finalize_reset())))?,
    );

    // serialize the modified input and checksum it
    let config_data = serde_json::to_vec_pretty(&val)?;
    hasher.update(&config_data);
    let checksum = format!("{:x}", hasher.finalize());
    info!(checksum = &checksum, "Output checksum");

    // validate that the input json would also deserialize to a Config
    _ = serde_json::from_value::<core::Config>(val.clone())?;
    info!("Configuration is valid");

    // write the checksum of the final config to the lockfile
    // for that config, ensuring that it can be used for comparison
    // when this new config is used in other commands
    let lockfile = core::ConfigLockfile::new(checksum);
    lockfile.save(&lockfile_path)?;
    fs::write(input.output_file_path, config_data)?;
    info!("Generated config and lockfile written");

    Ok(ConfigGenerateOutput { config: val })
}
