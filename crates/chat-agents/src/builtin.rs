use std::sync::Arc;

use serde_json::{json, Value};

use chat_sandbox::{ExecRequest, SandboxBackend};

use crate::tool::Tool;

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
        "Execute code in an isolated sandbox and return stdout/stderr and the exit code. \
         Network access is disabled. Use this for calculations, data processing and file generation."
    }

    fn schema(&self) -> Value {
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

    async fn call(&self, arguments: Value) -> anyhow::Result<String> {
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

        let request = ExecRequest {
            language,
            code,
            files: Vec::new(),
        };
        let result = self.sandbox.run(&request).await?;

        let mut output = format!(
            "exit_code={}\ntimed_out={}\nstdout:\n{}\nstderr:\n{}",
            result.exit_code, result.timed_out, result.stdout, result.stderr
        );
        if result.truncated {
            output.push_str("\n[output truncated at 1 MiB]");
        }
        Ok(output)
    }
}
