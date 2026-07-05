use crate::api::schema::{InputPromptParams, Request, ResponseResult};
use crate::app::state::InputPromptState;
use crate::app::{App, Mode};

use super::responses::{encode_error, encode_success};

/// A deferred `input.prompt` API request waiting for the user to act on the
/// inline input modal. The socket connection thread blocks on `respond_to`
/// until the prompt is submitted, cancelled, or dismissed.
pub(crate) struct PendingInputPrompt {
    pub(crate) id: String,
    pub(crate) respond_to: std::sync::mpsc::Sender<String>,
}

impl App {
    pub(crate) fn handle_deferred_input_prompt_api_request(
        &mut self,
        request: Request,
        respond_to: std::sync::mpsc::Sender<String>,
    ) -> bool {
        match request.method {
            crate::api::schema::Method::InputPrompt(params) => {
                self.start_api_input_prompt(request.id, params, respond_to);
                true
            }
            _ => false,
        }
    }

    fn start_api_input_prompt(
        &mut self,
        id: String,
        params: InputPromptParams,
        respond_to: std::sync::mpsc::Sender<String>,
    ) {
        let title = params.prompt.trim().to_string();
        if title.is_empty() {
            let _ = respond_to.send(encode_error(id, "invalid_request", "prompt is required"));
            return;
        }
        if self.pending_input_prompt.is_some()
            || !matches!(self.state.mode, Mode::Terminal | Mode::Navigate)
        {
            let _ = respond_to.send(encode_error(
                id,
                "modal_already_open",
                "another modal or input prompt is already open",
            ));
            return;
        }

        self.state.input_prompt = Some(InputPromptState { title });
        self.state.name_input.clear();
        self.state.name_input_replace_on_type = false;
        self.state.mode = Mode::InputPrompt;
        self.pending_input_prompt = Some(PendingInputPrompt { id, respond_to });
    }

    /// Resolve the pending prompt with the entered text and close the modal.
    pub(crate) fn submit_input_prompt(&mut self) {
        let value = self.state.name_input.trim().to_string();
        if let Some(pending) = self.pending_input_prompt.take() {
            let _ = pending.respond_to.send(encode_success(
                pending.id,
                ResponseResult::InputPrompt { value },
            ));
        }
        self.close_input_prompt_modal();
    }

    /// Resolve the pending prompt with a cancellation error and close the modal.
    pub(crate) fn cancel_input_prompt(&mut self) {
        if let Some(pending) = self.pending_input_prompt.take() {
            let _ = pending.respond_to.send(encode_error(
                pending.id,
                "input_prompt_cancelled",
                "input prompt was cancelled",
            ));
        }
        self.close_input_prompt_modal();
    }

    /// Safety net for requests or events that replace the prompt modal while a
    /// response is still pending (for example an API call that changes mode).
    /// Without this the socket connection thread would block forever.
    pub(crate) fn reconcile_displaced_input_prompt(&mut self) {
        if self.state.mode == Mode::InputPrompt {
            return;
        }
        if let Some(pending) = self.pending_input_prompt.take() {
            let _ = pending.respond_to.send(encode_error(
                pending.id,
                "input_prompt_dismissed",
                "input prompt was dismissed before the user responded",
            ));
            self.state.input_prompt = None;
        }
    }

    fn close_input_prompt_modal(&mut self) {
        self.state.input_prompt = None;
        self.state.name_input.clear();
        self.state.name_input_replace_on_type = false;
        if self.state.mode == Mode::InputPrompt {
            self.state.mode = if self.state.active.is_some() {
                Mode::Terminal
            } else {
                Mode::Navigate
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent};

    use crate::api::schema::{
        ErrorResponse, InputPromptParams, Method, Request, ResponseResult, SuccessResponse,
    };
    use crate::app::{App, Mode};
    use crate::config::Config;

    fn test_app() -> App {
        let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = App::new(
            &Config::default(),
            true,
            None,
            api_rx,
            crate::api::EventHub::default(),
        );
        // A fresh app starts in onboarding, which correctly rejects prompts;
        // start from the plain navigate baseline instead.
        app.state.mode = Mode::Navigate;
        app
    }

    fn prompt_request(prompt: &str) -> Request {
        Request {
            id: "req_input".into(),
            method: Method::InputPrompt(InputPromptParams {
                prompt: prompt.into(),
            }),
        }
    }

    fn open_prompt(app: &mut App, prompt: &str) -> std::sync::mpsc::Receiver<String> {
        let (respond_to, response_rx) = std::sync::mpsc::channel();
        assert!(app.handle_deferred_input_prompt_api_request(prompt_request(prompt), respond_to));
        response_rx
    }

    fn recv_error(response_rx: &std::sync::mpsc::Receiver<String>) -> ErrorResponse {
        let response = response_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("input prompt request should respond");
        serde_json::from_str(&response).expect("response should be an error")
    }

    #[tokio::test]
    async fn input_prompt_opens_modal_and_defers_response() {
        let mut app = test_app();
        let response_rx = open_prompt(&mut app, "Tab name");

        assert_eq!(app.state.mode, Mode::InputPrompt);
        assert_eq!(
            app.state.input_prompt.as_ref().map(|p| p.title.as_str()),
            Some("Tab name")
        );
        assert!(app.pending_input_prompt.is_some());
        assert!(response_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn input_prompt_submit_returns_typed_text() {
        let mut app = test_app();
        let response_rx = open_prompt(&mut app, "Tab name");

        for ch in "logs".chars() {
            app.handle_input_prompt_key(KeyEvent::from(KeyCode::Char(ch)));
        }
        app.handle_input_prompt_key(KeyEvent::from(KeyCode::Enter));

        let response = response_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("submit should resolve the deferred response");
        let success: SuccessResponse = serde_json::from_str(&response).unwrap();
        assert_eq!(
            success.result,
            ResponseResult::InputPrompt {
                value: "logs".into()
            }
        );
        assert_eq!(app.state.mode, Mode::Navigate);
        assert!(app.state.input_prompt.is_none());
        assert!(app.pending_input_prompt.is_none());
        assert!(app.state.name_input.is_empty());
    }

    #[tokio::test]
    async fn input_prompt_escape_cancels_with_structured_error() {
        let mut app = test_app();
        let response_rx = open_prompt(&mut app, "Tab name");

        app.handle_input_prompt_key(KeyEvent::from(KeyCode::Esc));

        let error = recv_error(&response_rx);
        assert_eq!(error.error.code, "input_prompt_cancelled");
        assert_eq!(app.state.mode, Mode::Navigate);
        assert!(app.state.input_prompt.is_none());
        assert!(app.pending_input_prompt.is_none());
    }

    #[tokio::test]
    async fn input_prompt_rejects_empty_prompt() {
        let mut app = test_app();
        let response_rx = open_prompt(&mut app, "   ");

        let error = recv_error(&response_rx);
        assert_eq!(error.error.code, "invalid_request");
        assert_eq!(app.state.mode, Mode::Navigate);
    }

    #[tokio::test]
    async fn input_prompt_rejects_second_prompt_while_pending() {
        let mut app = test_app();
        let first_rx = open_prompt(&mut app, "First");
        let second_rx = open_prompt(&mut app, "Second");

        let error = recv_error(&second_rx);
        assert_eq!(error.error.code, "modal_already_open");
        assert_eq!(
            app.state.input_prompt.as_ref().map(|p| p.title.as_str()),
            Some("First")
        );
        assert!(first_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn input_prompt_rejects_when_another_modal_is_open() {
        let mut app = test_app();
        app.state.mode = Mode::Settings;
        let response_rx = open_prompt(&mut app, "Tab name");

        let error = recv_error(&response_rx);
        assert_eq!(error.error.code, "modal_already_open");
        assert_eq!(app.state.mode, Mode::Settings);
        assert!(app.state.input_prompt.is_none());
    }

    #[tokio::test]
    async fn displaced_input_prompt_resolves_with_dismissed_error() {
        let mut app = test_app();
        let response_rx = open_prompt(&mut app, "Tab name");

        // Simulate an API request or internal event stealing the modal mode.
        app.state.mode = Mode::Navigate;
        app.reconcile_displaced_input_prompt();

        let error = recv_error(&response_rx);
        assert_eq!(error.error.code, "input_prompt_dismissed");
        assert!(app.state.input_prompt.is_none());
        assert!(app.pending_input_prompt.is_none());
    }
}
