use std::sync::Arc;

use serde_json::{json, Value};
use uuid::Uuid;

use chat_sandbox::{ExecRequest, ExecResult, SandboxBackend};

use crate::tool::{Tool, UserSandbox};

const DESCRIPTION: &str =
    "Execute code in an isolated Linux sandbox (no network access) and return stdout, stderr and \
     the exit code. Call this whenever a task needs computation, data analysis or transformation, \
     or when producing a file — never guess numeric results or claim to have run code without \
     calling this tool. The conversation's attached files are already available in the working \
     directory /app; write any file you want to hand back to the user into /app/output so it is \
     saved to their file library. Supported languages include python, javascript, bash, go and rust.";

const PERSISTENT_DESCRIPTION: &str =
    "Execute code in the caller's persistent sandbox workspace: an isolated Linux environment \
     (no network access) where files and installed packages persist between calls, so this is the \
     place to do multi-step work. Call this whenever a task needs computation, data analysis or \
     transformation, or when producing a file — never guess numeric results or claim to have run \
     code without calling this tool. The conversation's attached files are already available in \
     /app; write any file you want to hand back to the user into /app/output so it is saved to \
     their file library. Supported languages include python, javascript, bash, go and rust.";

/// A tool that runs model-generated code in the configured [`SandboxBackend`].
pub struct CodeInterpreterTool {
    sandbox: Arc<dyn SandboxBackend>,
}

impl CodeInterpreterTool {
    pub fn new(sandbox: Arc<dyn SandboxBackend>) -> Self {
        Self { sandbox }
    }
}

#[async_trait::async_trait]
impl Tool for CodeInterpreterTool {
    fn name(&self) -> &str {
        "execute_code"
    }

    fn description(&self) -> &str {
        DESCRIPTION
    }

    fn schema(&self) -> Value {
        schema()
    }

    async fn call(&self, arguments: Value) -> anyhow::Result<String> {
        let request = parse_request(&arguments)?;
        let result = self.sandbox.run(&request).await?;
        Ok(format_result(result))
    }
}

/// A code interpreter bound to one user's persistent workspace.
///
/// Unlike [`CodeInterpreterTool`], this delegates through a [`UserSandbox`] so
/// the code runs in the caller's own long-lived sandbox rather than a shared,
/// throwaway one. The server constructs it per request.
pub struct UserCodeInterpreterTool<S: UserSandbox> {
    sandbox: Arc<S>,
    user_id: Uuid,
}

impl<S: UserSandbox> UserCodeInterpreterTool<S> {
    pub fn new(sandbox: Arc<S>, user_id: Uuid) -> Self {
        Self { sandbox, user_id }
    }
}

#[async_trait::async_trait]
impl<S: UserSandbox> Tool for UserCodeInterpreterTool<S> {
    fn name(&self) -> &str {
        "execute_code"
    }

    fn description(&self) -> &str {
        PERSISTENT_DESCRIPTION
    }

    fn schema(&self) -> Value {
        schema()
    }

    async fn call(&self, arguments: Value) -> anyhow::Result<String> {
        let request = parse_request(&arguments)?;
        let result = self.sandbox.exec_for_user(self.user_id, &request).await?;
        Ok(format_result(result))
    }
}

fn schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "language": {
                "type": "string",
                "description": "Programming language, e.g. python, javascript, bash, go, rust"
            },
            "code": { "type": "string", "description": "Source code to execute" }
        },
        "required": ["language", "code"]
    })
}

/// Parse the shared `{ language, code }` tool arguments.
fn parse_request(arguments: &Value) -> anyhow::Result<ExecRequest> {
    let language = arguments
        .get("language")
        .and_then(Value::as_str)
        .unwrap_or("python")
        .to_string();
    let code = arguments
        .get("code")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("missing `code` argument"))?
        .to_string();
    Ok(ExecRequest {
        language,
        code,
        files: Vec::new(),
        outputs: Vec::new(),
    })
}

/// Render an [`ExecResult`] as model-facing text.
fn format_result(result: ExecResult) -> String {
    let mut output = format!(
        "exit_code={}\ntimed_out={}\nstdout:\n{}\nstderr:\n{}",
        result.exit_code, result.timed_out, result.stdout, result.stderr
    );
    if result.truncated {
        output.push_str("\n[output truncated at 1 MiB]");
    }
    if !result.files.is_empty() {
        let names: Vec<&str> = result.files.iter().map(|f| f.name.as_str()).collect();
        output.push_str(&format!(
            "\n[saved {} output file(s) to the user's storage: {}]",
            names.len(),
            names.join(", ")
        ));
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Records the `(user_id, language)` of the last execution.
    #[derive(Default)]
    struct MockUserSandbox {
        seen: Mutex<Vec<(Uuid, String)>>,
    }

    #[async_trait::async_trait]
    impl UserSandbox for MockUserSandbox {
        async fn exec_for_user(
            &self,
            user_id: Uuid,
            request: &ExecRequest,
        ) -> anyhow::Result<ExecResult> {
            self.seen
                .lock()
                .unwrap()
                .push((user_id, request.language.clone()));
            Ok(ExecResult {
                exit_code: 0,
                stdout: "hi".into(),
                stderr: String::new(),
                timed_out: false,
                truncated: false,
                files: Vec::new(),
            })
        }
    }

    #[tokio::test]
    async fn user_code_interpreter_runs_for_the_bound_user() {
        let sandbox = Arc::new(MockUserSandbox::default());
        let user = Uuid::new_v4();
        let tool = UserCodeInterpreterTool::new(sandbox.clone(), user);

        assert_eq!(tool.name(), "execute_code");
        let output = tool
            .call(json!({ "language": "bash", "code": "echo hi" }))
            .await
            .unwrap();

        assert!(output.contains("stdout:\nhi"), "{output}");
        assert_eq!(
            sandbox.seen.lock().unwrap().as_slice(),
            &[(user, "bash".into())]
        );
    }

    #[tokio::test]
    async fn missing_code_is_rejected_before_executing() {
        let sandbox = Arc::new(MockUserSandbox::default());
        let tool = UserCodeInterpreterTool::new(sandbox.clone(), Uuid::new_v4());
        assert!(tool.call(json!({ "language": "python" })).await.is_err());
        assert!(sandbox.seen.lock().unwrap().is_empty());
    }
}
