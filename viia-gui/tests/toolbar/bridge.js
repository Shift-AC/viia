// Test-only transport. The HTML/CSS/handlers under test come directly from the production frontend.
(() => {
  const listeners = new Map();
  const calls = [];
  let resolveReady;
  const ready = new Promise(resolve => resolveReady = resolve);
  const emit = (name, payload) => listeners.get(name)?.({ payload });
  let meta = {
    current_index: 12, total: 48, display_text: 'alpine-lake_012.jpg',
    file_name: 'alpine-lake_012.jpg', source_url: 'file:///C:/images/alpine-lake_012.jpg',
    playback_state: 'Paused'
  };
  const startup = document.documentElement.dataset.testStartup;
  if (startup === 'empty' || startup === 'missing') {
    meta = { current_index: 0, total: 0, display_text: 'No images',
      file_name: null, source_url: null, playback_state: 'Paused' };
  }
  const setMeta = value => { meta = { ...meta, ...value }; emit('status-meta', meta); };
  const win = {
    onResized: async () => {}, onDragDropEvent: async () => {},
    isMaximized: async () => false, isFullscreen: async () => false,
    startDragging: async () => {}, setFullscreen: async () => {}, show: async () => {},
    minimize: async () => calls.push({ command: 'window.minimize' }),
    toggleMaximize: async () => calls.push({ command: 'window.maximize' })
  };
  window.__TAURI__ = {
    core: {
      convertFileSrc: path => path,
      invoke: async (command, args = {}) => {
        if (command === 'log_to_terminal') return;
        calls.push({ command, ...args });
        if (command === 'start_engine') {
          if (startup !== 'missing') setMeta({});
          resolveReady();
        }
        if (command === 'send_command') {
          if (args.cmd === 'p') setMeta({ playback_state: meta.playback_state === 'Playing' ? 'Paused' : 'Playing' });
          if (args.cmd === 'a L1T3') setMeta({ playback_state: 'Playing' });
        }
      }
    },
    event: { listen: async (name, callback) => { listeners.set(name, callback); return () => listeners.delete(name); } },
    window: { getCurrentWindow: () => win }
  };
  window.toolbarTest = { ready, calls, setMeta, emit };
})();
