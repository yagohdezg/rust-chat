use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use chat_mcp::StdioMcpClient;
use serde_json::json;

const DELAY_SERVER_JS: &str = r#"
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
    reply({ protocolVersion: '2025-06-18', capabilities: { tools: {} }, serverInfo: { name: 'delay', version: '0' } });
  } else if (msg.method === 'tools/list') {
    reply({ tools: [{ name: 'delay', description: 'Replies after a delay', inputSchema: { type: 'object', properties: { text: { type: 'string' }, ms: { type: 'number' } } } }] });
  } else if (msg.method === 'tools/call') {
    const args = (msg.params && msg.params.arguments) || {};
    const text = args.text || '';
    const ms = args.ms || 0;
    setTimeout(() => reply({ content: [{ type: 'text', text }], isError: false }), ms);
  } else {
    process.stdout.write(JSON.stringify({ jsonrpc: '2.0', id: msg.id, error: { code: -32601, message: 'method not found' } }) + '\n');
  }
});
"#;

fn node_available() -> bool {
    std::process::Command::new("node")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

#[tokio::test]
async fn concurrent_calls_are_correlated_and_do_not_block() {
    if !node_available() {
        eprintln!("skipping: node is not available");
        return;
    }

    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("rustchat-mcp-{stamp}"));
    std::fs::create_dir_all(&dir).unwrap();
    let script = dir.join("delay-mcp.cjs");
    std::fs::write(&script, DELAY_SERVER_JS).unwrap();

    let client = StdioMcpClient::spawn("node", &[script.display().to_string()], &BTreeMap::new())
        .await
        .unwrap();
    client.initialize().await.unwrap();
    assert_eq!(client.list_tools().await.unwrap().len(), 1);

    let (slow, fast) = tokio::join!(
        async {
            let start = Instant::now();
            let result = client
                .call_tool("delay", json!({ "text": "slow", "ms": 800 }))
                .await
                .unwrap();
            (result, start.elapsed())
        },
        async {
            tokio::time::sleep(Duration::from_millis(100)).await;
            let start = Instant::now();
            let result = client
                .call_tool("delay", json!({ "text": "fast", "ms": 0 }))
                .await
                .unwrap();
            (result, start.elapsed())
        },
    );

    let (slow_result, slow_elapsed) = slow;
    let (fast_result, fast_elapsed) = fast;

    assert_eq!(slow_result["content"][0]["text"], "slow");
    assert_eq!(fast_result["content"][0]["text"], "fast");
    assert!(
        fast_elapsed < Duration::from_millis(500),
        "the fast call waited on the slow one: {fast_elapsed:?}"
    );
    assert!(
        slow_elapsed >= Duration::from_millis(700),
        "the slow call returned before its delay: {slow_elapsed:?}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
