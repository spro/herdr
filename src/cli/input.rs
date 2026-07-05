use crate::api::client::ApiClientError;
use crate::api::schema::{InputPromptParams, Method, Request, ResponseResult};

const INPUT_USAGE: &str = "usage: herdr input --prompt TEXT";

pub(super) fn run_input_command(args: &[String]) -> std::io::Result<i32> {
    let params = match parse_input_args(args) {
        Ok(params) => params,
        Err(InputArgError::Usage) => {
            eprintln!("{INPUT_USAGE}");
            return Ok(2);
        }
        Err(InputArgError::Help) => {
            print_input_help();
            return Ok(0);
        }
        Err(InputArgError::Message(message)) => {
            eprintln!("{message}");
            return Ok(2);
        }
    };

    let response = super::send_request(&Request {
        id: "cli:input:prompt".into(),
        method: Method::InputPrompt(params),
    })?;

    match crate::api::client::parse_response_value(response) {
        Ok(success) => {
            let ResponseResult::InputPrompt { value } = success.result else {
                return Err(std::io::Error::other("unexpected input prompt result"));
            };
            println!("{value}");
            Ok(0)
        }
        Err(ApiClientError::ErrorResponse(response)) => {
            match response.error.code.as_str() {
                "input_prompt_cancelled" => eprintln!("input prompt was cancelled"),
                _ => eprintln!(
                    "{}",
                    serde_json::to_string(&response).map_err(std::io::Error::other)?
                ),
            }
            Ok(1)
        }
        Err(err) => Err(std::io::Error::other(err)),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum InputArgError {
    Usage,
    Help,
    Message(String),
}

fn parse_input_args(args: &[String]) -> Result<InputPromptParams, InputArgError> {
    let mut prompt = None;

    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--prompt" => {
                let Some(value) = args.get(index + 1) else {
                    return Err(InputArgError::Message("missing value for --prompt".into()));
                };
                prompt = Some(value.clone());
                index += 2;
            }
            "help" | "--help" | "-h" => return Err(InputArgError::Help),
            other => {
                return Err(InputArgError::Message(format!("unknown option: {other}")));
            }
        }
    }

    let Some(prompt) = prompt else {
        return Err(InputArgError::Usage);
    };
    if prompt.trim().is_empty() {
        return Err(InputArgError::Message("prompt must not be empty".into()));
    }

    Ok(InputPromptParams { prompt })
}

fn print_input_help() {
    eprintln!("herdr input:");
    eprintln!("  {INPUT_USAGE}");
    eprintln!();
    eprintln!("Shows an inline input prompt in the Herdr UI and prints the");
    eprintln!("entered text to stdout once the user submits it.");
    eprintln!();
    eprintln!("  --prompt TEXT   prompt title shown in the modal (required)");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    #[test]
    fn input_args_parse_prompt() {
        let params = parse_input_args(&args(&["--prompt", "Tab name"])).unwrap();

        assert_eq!(
            params,
            InputPromptParams {
                prompt: "Tab name".into(),
            }
        );
    }

    #[test]
    fn input_args_require_prompt() {
        assert_eq!(parse_input_args(&args(&[])), Err(InputArgError::Usage));
    }
}
