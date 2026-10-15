// zing-explore 的 Lua 除錯：以具名管道連到 zing 內建的除錯器（DAP，見 crates/zing-app/src/luadebug.rs）。
// 管道名稱必須與 luadebug::pipe_name 相同。
const vscode = require('vscode');

function pipeName() {
  const user = (process.env.USERNAME || '').replace(/[^\p{L}\p{N}_-]/gu, '_');
  return `\\\\.\\pipe\\zing-lua-debug-${user}`;
}

function activate(context) {
  context.subscriptions.push(
    vscode.debug.registerDebugAdapterDescriptorFactory('zing-lua', {
      createDebugAdapterDescriptor() {
        return new vscode.DebugAdapterNamedPipeServer(pipeName());
      },
    }),
    // 沒有 launch.json 時按 F5 也能直接附加
    vscode.debug.registerDebugConfigurationProvider('zing-lua', {
      resolveDebugConfiguration(_folder, config) {
        if (!config.type && !config.request) {
          return { type: 'zing-lua', request: 'attach', name: '附加到 zing' };
        }
        return config;
      },
    }),
  );
}

function deactivate() {}

module.exports = { activate, deactivate };
