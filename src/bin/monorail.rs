use monorail::cli;
use monorail::tracing;

#[tokio::main]
async fn main() {
    let app = cli::get_app();
    let matches = app.get_matches();
    let format = matches.get_one::<String>(cli::ARG_OUTPUT_FORMAT).unwrap();
    let verbosity = matches.get_one::<u8>(cli::ARG_VERBOSE).unwrap_or(&0);
    let output_options = cli::OutputOptions { format };

    tracing::setup(format, *verbosity).unwrap();

    match cli::handle(&matches, &output_options).await {
        Ok(code) => {
            std::process::exit(code);
        }
        Err(e) => {
            cli::write_result::<()>(&Err(e), &output_options)
                .expect("Failed to write fatal result");
            std::process::exit(cli::HANDLE_FATAL);
        }
    }
}
