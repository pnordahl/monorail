use monorail::api::cli;

fn main() {
    let app = cli::build();
    let matches = app.get_matches();
    let format = matches.get_one::<String>(cli::ARG_OUTPUT_FORMAT).unwrap();
    let output_options = cli::OutputOptions { format };

    match cli::handle(&matches, &output_options) {
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
