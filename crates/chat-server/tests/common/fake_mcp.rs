use std::path::Path;

/// A minimal MCP stdio server (Node) that exposes a single `echo` tool.
///
/// It speaks newline-delimited JSON-RPC: `initialize`, `tools/list` and
/// `tools/call` are answered; notifications are ignored.
pub const ECHO_SERVER_JS: &str = r#"
const readline = require('readline');
const rl = readline.createInterface({ input: process.stdin });
rl.on('close', () => process.exit(0));
rl.on('line', (line) => {
  line = line.trim();
  if (!line) return;
  let msg;
  try { msg = JSON.parse(line); } catch { return; }
  if (msg.id === undefined) return;
  const reply = (result) => process.stdout.write(JSON.stringify({ jsonrpc: '2.0', id: msg.id, result }) + '\n');
  if (msg.method === 'initialize') {
    reply({ protocolVersion: '2025-06-18', capabilities: { tools: {} }, serverInfo: { name: 'echo', version: '0' } });
  } else if (msg.method === 'tools/list') {
    reply({ tools: [{ name: 'echo', description: 'Echoes the given text', inputSchema: { type: 'object', properties: { text: { type: 'string' } }, required: ['text'] } }] });
  } else if (msg.method === 'tools/call') {
    const text = (msg.params && msg.params.arguments && msg.params.arguments.text) || '';
    reply({ content: [{ type: 'text', text }], isError: false });
  } else {
    process.stdout.write(JSON.stringify({ jsonrpc: '2.0', id: msg.id, error: { code: -32601, message: 'method not found' } }) + '\n');
  }
});
"#;

/// Whether `node` is on `PATH`, so the MCP fixture can be skipped when it is not.
pub fn node_available() -> bool {
    std::process::Command::new("node")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// Write the echo MCP server into `dir` and return the `(command, args)` to
/// spawn it.
pub fn write_echo_server(dir: &Path) -> (String, Vec<String>) {
    let script = dir.join("echo-mcp.cjs");
    std::fs::write(&script, ECHO_SERVER_JS).unwrap();
    ("node".to_string(), vec![script.display().to_string()])
}
