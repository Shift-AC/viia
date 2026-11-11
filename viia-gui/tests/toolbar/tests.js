// Isolated GUI unit tests with a mocked Tauri transport.
(() => {
  const $ = id => document.getElementById(id);
  const visible = id => $(id).getClientRects().length > 0;
  const near = (actual, expected, label) => {
    if (Math.abs(actual - expected) > 1) throw new Error(`${label}: ${actual}, expected ${expected}`);
  };
  const check = (condition, label) => { if (!condition) throw new Error(label); };
  const key = (element, value) => element.dispatchEvent(new KeyboardEvent('keydown', { key: value, bubbles: true, cancelable: true }));
  const helpFixture = 'viia 0.1.5 (build: test)\nCommands:\n  g <INDEX>  Go to an image\n  a <SPEC>  Play an animation\n  help  Print this message\n';
  const matchFixture = { pattern: '[图像]*', rows: [{ index: 12345, name: '图像 [final] '.repeat(100) + '.webp' }] };
  async function startupFrame(startup) {
    const frame = document.createElement('iframe');
    frame.style.cssText = 'position:fixed;width:800px;height:600px;opacity:0;pointer-events:none;border:0;';
    let html = await (await fetch('../../dist/index.html')).text();
    html = html.replace('<html lang="en">', `<html lang="en" data-test-startup="${startup}">`);
    const loaded = new Promise(resolve => frame.onload = resolve);
    frame.srcdoc = html.replace('<script>', '<script src="/viia-gui/tests/toolbar/bridge.js"><\/script><script>');
    document.body.appendChild(frame);
    await loaded;
    await frame.contentWindow.toolbarTest.ready;
    return frame;
  }
  const reset = () => {
    for (const id of ['btn-close-url', 'btn-close-sort', 'btn-close-help', 'btn-close-match']) $(id).click();
    $('file-menu').hidden = true;
    $('command-prompt').style.display = 'none';
    $('command-input').value = '';
    document.activeElement?.blur();
    for (const item of [...$('sort-list').children].slice(1)) item.remove();
    $('sort-list').querySelector('select').value = 'name';
    if ($('overlay').classList.contains('minimized')) $('overlay-drag-bar').dispatchEvent(new MouseEvent('dblclick', { bubbles: true }));
    if ($('overlay').classList.contains('full')) $('toolbar-toggle').click();
    for (const property of ['left', 'top', 'bottom', 'transform']) $('overlay').style[property] = '';
    toolbarTest.setMeta({
      current_index: 12, total: 48, display_text: 'alpine-lake_012.jpg',
      file_name: 'alpine-lake_012.jpg', source_url: 'file:///C:/images/alpine-lake_012.jpg',
      playback_state: 'Paused'
    });
    toolbarTest.calls.length = 0;
  };
  const tests = [
    ['Native context menus are suppressed across viewer, controls, and popups', () => {
      for (const target of [document.body, $('viewer'), $('viewer-canvas'), $('btn-pause'),
        $('toolbar-toggle'), $('index-input'), $('url-input'), $('command-input'), $('file-menu')]) {
        for (const button of [2, 0]) {
          const event = new MouseEvent('contextmenu', { bubbles: true, cancelable: true, button });
          check(!target.dispatchEvent(event) && event.defaultPrevented, 'Mouse and keyboard context menus must be canceled');
        }
      }
      check(toolbarTest.calls.length === 0, 'Suppressing context menus must not send backend commands');
      $('toolbar-toggle').click();
      $('status-msg').click();
      check(visible('file-menu'), 'The app file-action menu must still open on left click');
    }],
    ['Empty startup initializes full controls and local spacing even without status metadata', async () => {
      for (const startup of ['empty', 'missing']) {
        const frame = await startupFrame(startup);
        try {
          const doc = frame.contentDocument, el = id => doc.getElementById(id);
          check(el('overlay').classList.contains('full'), `${startup}: startup must show full toolbar`);
          check(!el('btn-open-file').disabled && !el('btn-open-url').disabled && !el('btn-cmd').disabled, 'Opening utilities available');
          check(el('btn-pause').disabled && el('btn-sort').disabled && el('index-input').disabled && el('scale-input').disabled, 'Image controls disabled');
          check(el('index-input').value === '0' && el('index-total').textContent === '/ 0' && el('scale-input').value === '100', 'Local zero-image labels');
          const input = el('index-input').getBoundingClientRect(), total = el('index-total').getBoundingClientRect();
          check(input.width < 30 && total.left - input.right <= 3, 'Zero index must have compact local spacing');
          el('toolbar-toggle').click();
          check(el('overlay').offsetWidth < 250, 'Empty compact toolbar must not use browser-default input widths');
        } finally { frame.remove(); }
      }
    }],
    ['Startup with images defaults to compact and subsequent status preserves the chosen mode', async () => {
      const frame = await startupFrame('images');
      try {
        const doc = frame.contentDocument, el = id => doc.getElementById(id), bridge = frame.contentWindow.toolbarTest;
        check(!el('overlay').classList.contains('full'), 'Supplied images start compact');
        el('toolbar-toggle').click();
        bridge.setMeta({ current_index: 0, total: 0, file_name: null, source_url: null });
        check(el('overlay').classList.contains('full'), 'Empty status preserves chosen full mode');
        bridge.setMeta({ current_index: 1, total: 1, file_name: 'image.png', source_url: 'file:///C:/image.png' });
        check(el('overlay').classList.contains('full') && !el('btn-pause').disabled, 'Loading images preserves chosen mode and enables controls');
      } finally { frame.remove(); }
    }],
    ['A toolbar choice before the first status is not overwritten by late metadata', async () => {
      const frame = await startupFrame('missing');
      try {
        const el = id => frame.contentDocument.getElementById(id);
        el('toolbar-toggle').click();
        frame.contentWindow.toolbarTest.setMeta({ total: 0 });
        check(!el('overlay').classList.contains('full'), 'Late empty startup metadata must preserve compact choice');
      } finally { frame.remove(); }
    }],
    ['Compact default and playback command bindings', async () => {
      check(!visible('status-msg') && !visible('mode-text') && !visible('btn-open-file'), 'Only hot controls should be visible');
      for (const [id, cmd] of [['btn-prev', 'l'], ['btn-pause', 'p'], ['btn-next', 'r'], ['btn-slideshow', 'a L1T3']]) {
        check(visible(id), `${id} must remain visible`);
        $(id).click();
        check(toolbarTest.calls.at(-1)?.cmd === cmd, `${id} command mismatch`);
      }
      check($('icon-pause').style.display === 'block', 'Playback state must drive the pause icon');
    }],
    ['Index and zoom edits reach the existing backend', async () => {
      $('index-input').value = '24';
      key($('index-input'), 'Enter');
      await Promise.resolve();
      check(toolbarTest.calls.some(call => call.command === 'send_command' && call.cmd === 'g 24'), 'Index edit command');
      $('scale-input').value = '125';
      key($('scale-input'), 'Enter');
      check(toolbarTest.calls.some(call => call.cmd === 'z 1.25'), 'Zoom edit command');
    }],
    ['Compact window controls stack before expand and preserve window actions in both modes', () => {
      const ids = ['btn-minimize', 'btn-maximize', 'btn-close'];
      const commands = ['window.minimize', 'window.maximize', 'send_command'];
      const content = $('overlay-content').getBoundingClientRect();
      const toggle = $('toolbar-toggle').getBoundingClientRect();
      const playback = $('btn-pause').getBoundingClientRect();
      const rects = ids.map(id => $(id).getBoundingClientRect());
      for (let i = 0; i < ids.length; i++) {
        check(visible(ids[i]) && !$(ids[i]).disabled, `${ids[i]} must be available in compact mode`);
        near(rects[i].width, 24, 'Compact window button width');
        near(rects[i].height, 20, 'Compact window button height');
        check(rects[i].width < playback.width && rects[i].height < playback.height, 'Window controls must be smaller than playback');
        check(rects[i].left >= playback.right && rects[i].right <= toggle.left, 'Window controls must sit before expand, after playback');
        check(rects[i].top >= content.top && rects[i].bottom <= content.bottom, 'Window controls must fit both rows');
        if (i) {
          near(rects[i].left, rects[0].left, 'Window column alignment');
          check(rects[i].top >= rects[i - 1].bottom, 'Window controls must stack without overlap');
        }
      }
      for (const full of [false, true]) {
        if (full) $('toolbar-toggle').click();
        for (let i = 0; i < ids.length; i++) {
          $(ids[i]).click();
          check(toolbarTest.calls.at(-1)?.command === commands[i], `${ids[i]} must preserve its existing action`);
        }
        check(toolbarTest.calls.at(-1)?.cmd === 'q', 'Close must use the terminal quit command');
        if (full) {
          const first = $(ids[0]).getBoundingClientRect();
          const last = $(ids[2]).getBoundingClientRect();
          near(first.top, last.top, 'Full window controls stay horizontal');
          near(first.width, 32, 'Full window button width');
        }
      }
      toolbarTest.setMeta({ current_index: 0, total: 0, file_name: null, source_url: null });
      $('toolbar-toggle').click();
      check(ids.every(id => visible(id) && !$(id).disabled), 'Empty compact state must keep window controls enabled');
    }],
    ['Large indices resize compact mode and restore its width', () => {
      const before = $('overlay').offsetWidth;
      toolbarTest.setMeta({ current_index: 12345, total: 12480 });
      check($('overlay').offsetWidth > before, 'Large index should grow the toolbar');
      check($('index-input').value === '12345', 'Large index must not truncate its value');
      toolbarTest.setMeta({ current_index: 12, total: 48 });
      near($('overlay').offsetWidth, before, 'Short index width restored');
    }],
    ['The centered drag indicator remains inside the draggable hit area', () => {
      const handle = $('overlay-drag-bar').getBoundingClientRect();
      const center = { x: handle.left + handle.width / 2, y: handle.top + handle.height / 2 + 0.5 };
      check(document.elementFromPoint(center.x, center.y) === $('overlay-drag-bar'), 'The visible grip must receive drag input');
      near($('overlay').offsetHeight, 78, 'Toolbar height remains compact');
    }],
    ['Long filenames cannot widen full mode or displace its controls', () => {
      $('toolbar-toggle').click();
      const width = $('overlay').offsetWidth;
      const long = '图像 [final] '.repeat(100) + '.webp';
      toolbarTest.setMeta({ display_text: long, file_name: long });
      near($('overlay').offsetWidth, width, 'Full toolbar width');
      check($('status-msg').scrollWidth > $('status-msg').clientWidth, 'Long filename must be truncated');
      check(visible('btn-open-file') && visible('btn-close'), 'All utilities remain accessible');
      check($('btn-open-file').closest('#button-row'), 'Utilities must be on the lower row');
      const content = $('overlay-content').getBoundingClientRect(), mode = $('mode-text').getBoundingClientRect();
      near(content.right - mode.right, 16, 'Mode label right padding');
    }],
    ['Minimization preserves mode, opacity, and bottom anchoring', () => {
      $('toolbar-toggle').click();
      key($('transparency-drag-bar'), 'Home');
      key($('transparency-drag-bar'), 'ArrowUp');
      const before = $('overlay').getBoundingClientRect();
      $('overlay-drag-bar').dispatchEvent(new MouseEvent('dblclick', { bubbles: true }));
      check(!visible('toolbar-toggle') && !visible('transparency-drag-bar'), 'Minimized toolbar contains only its handle');
      $('overlay-drag-bar').dispatchEvent(new MouseEvent('dblclick', { bubbles: true }));
      check($('overlay').classList.contains('full'), 'Full mode must survive minimization');
      check($('transparency-drag-bar').getAttribute('aria-valuenow') === '15', 'Opacity must survive minimization');
      near($('overlay').getBoundingClientRect().bottom, before.bottom, 'Bottom anchoring');
      $('toolbar-toggle').click();
      check($('transparency-drag-bar').getAttribute('aria-valuenow') === '15', 'Opacity must survive mode switch');
    }],
    ['Switching a moved toolbar preserves its center and keeps it visible', () => {
      $('overlay').style.left = '200px';
      $('overlay').style.top = '200px';
      $('overlay').style.bottom = 'auto';
      $('overlay').style.transform = 'none';
      const before = $('overlay').getBoundingClientRect();
      $('toolbar-toggle').click();
      const after = $('overlay').getBoundingClientRect();
      near(after.left + after.width / 2, before.left + before.width / 2, 'Toolbar center');
      near(after.top, before.top, 'Toolbar top');
      check(after.left >= 0 && after.right <= innerWidth, 'Expanded toolbar stays visible');
    }],
    ['Keyboard toolbar controls do not trigger a second pause command', () => {
      key($('toolbar-toggle'), ' ');
      key($('overlay-drag-bar'), 'Enter');
      check($('overlay').classList.contains('minimized'), 'Keyboard minimization');
      key($('overlay-drag-bar'), 'Enter');
      key($('transparency-drag-bar'), 'ArrowDown');
      key($('btn-next'), ' ');
      check(!toolbarTest.calls.some(call => call.cmd === 'p'), 'Toolbar key handling must not reach global pause');
    }],
    ['Empty state keeps the toolbar switch and file-opening utilities available', () => {
      toolbarTest.setMeta({ current_index: 0, total: 0, file_name: null, source_url: null, display_text: 'No images' });
      check($('btn-pause').disabled && $('index-input').disabled, 'Image controls must be disabled when empty');
      check(!$('toolbar-toggle').disabled, 'Toolbar switch must remain enabled when empty');
      $('toolbar-toggle').click();
      check(visible('btn-open-file') && !$('btn-open-file').disabled, 'Full mode can open files when empty');
    }],
    ['Compact URL dialog preserves multiline Unicode command submission and Escape', async () => {
      $('toolbar-toggle').click();
      $('btn-open-url').click();
      check(document.activeElement === $('url-input'), 'URL input must receive focus');
      $('url-input').value = ' file:///C:/图像 [final]/image.png \n\n https://example.com/a%20b.webp ';
      $('btn-submit-url').click();
      await Promise.resolve();
      check(toolbarTest.calls.at(-1)?.cmd === 'o "file:///C:/图像 [final]/image.png" "https://example.com/a%20b.webp"', 'URLs must retain quoting and ordering');
      check(!visible('url-modal') && $('url-input').value === '', 'Submission must close and clear');
      $('btn-open-url').click();
      key($('url-input'), 'Escape');
      check(!visible('url-modal'), 'Escape must close URL dialog');
    }],
    ['Sort priorities can be removed, renumbered, and applied in order', async () => {
      $('toolbar-toggle').click();
      $('btn-sort').click();
      $('btn-add-sort').click();
      $('btn-add-sort').click();
      $('sort-list').children[1].querySelector('.btn-remove-sort').click();
      check($('sort-list').children[1].querySelector('.sort-number').textContent === '2.', 'Remaining sort criterion must be renumbered');
      $('sort-list').querySelectorAll('select')[0].value = 'name-natural';
      $('sort-list').querySelectorAll('select')[1].value = 'modified-desc';
      $('btn-apply-sort').click();
      await Promise.resolve();
      check(toolbarTest.calls.at(-1)?.cmd === 's name-natural modified-desc', 'Sort priority order');
      check(!visible('sort-modal'), 'Apply must close sort dialog');
    }],
    ['Help and match results preserve backend text and dismiss without playback commands', () => {
      toolbarTest.emit('help', helpFixture);
      check(visible('help-modal') && $('help-terminal-commands').children.length === 2, 'Help parser must retain useful commands');
      check($('help-version-info').textContent.includes('build: test'), 'Help build information');
      key(document.body, 'Escape');
      check(!visible('help-modal'), 'Escape closes help');
      toolbarTest.emit('match-results', matchFixture);
      check($('match-table-body').rows[0].cells[1].textContent === matchFixture.rows[0].name, 'Long Unicode filename preserved');
      check($('match-content').scrollWidth <= $('match-content').clientWidth + 1, 'Match filenames must wrap without horizontal overflow');
      key(document.body, 'Escape');
      check(!visible('match-modal'), 'Escape closes matches');
      toolbarTest.emit('match-results', { pattern: 'none', rows: [] });
      check($('match-table-body').textContent === 'No files matched.', 'Empty match feedback');
      check(toolbarTest.calls.length === 0, 'Popup dismissal must not trigger playback');
    }],
    ['File menu and command prompt remain compact and actionable above the toolbar', async () => {
      $('toolbar-toggle').click();
      $('status-msg').click();
      check(visible('file-menu'), 'File menu opens');
      check(!$('btn-reveal-file').disabled, 'Local file can be revealed');
      $('btn-copy-file-name').click();
      await Promise.resolve();
      check(toolbarTest.calls.at(-1)?.command === 'plugin:clipboard-manager|write_text' && toolbarTest.calls.at(-1)?.text === 'alpine-lake_012.jpg', 'Copy action uses native backend');
      key(document.body, 'Escape');
      toolbarTest.setMeta({ source_url: 'https://example.com/image.webp' });
      $('status-msg').click();
      check($('btn-reveal-file').disabled, 'Remote file cannot be revealed');
      key(document.body, 'Escape');
      $('btn-cmd').click();
      check(Number(getComputedStyle($('command-prompt')).zIndex) > Number(getComputedStyle($('overlay')).zIndex), 'Command input must appear above toolbar');
      $('command-input').value = 'g 24';
      key($('command-input'), 'Enter');
      await Promise.resolve();
      check(toolbarTest.calls.at(-1)?.cmd === 'g 24' && !visible('command-prompt'), 'Command submission');
    }],
    ['Popups fit a small viewport and keep long content scrollable', async () => {
      const frame = document.createElement('iframe');
      frame.style.cssText = 'position:fixed;width:320px;height:260px;opacity:0;pointer-events:none;border:0;';
      const html = await (await fetch('../../dist/index.html')).text();
      const loaded = new Promise(resolve => frame.onload = resolve);
      frame.srcdoc = html.replace('<script>', '<script src="/viia-gui/tests/toolbar/bridge.js"><\/script><script>');
      document.body.appendChild(frame);
      try {
        await loaded;
        const win = frame.contentWindow, doc = win.document, el = id => doc.getElementById(id);
        await win.toolbarTest.ready;
        el('toolbar-toggle').click();
        const fits = id => {
          const rect = el(id).getBoundingClientRect();
          check(rect.left >= 0 && rect.top >= 0 && rect.right <= win.innerWidth + 1 && rect.bottom <= win.innerHeight + 1, `${id} must fit small viewport`);
        };
        el('btn-open-url').click();
        fits('url-modal');
        el('btn-close-url').click();
        el('btn-sort').click();
        for (let i = 0; i < 15; i++) el('btn-add-sort').click();
        fits('sort-modal');
        check(el('sort-content').scrollHeight > el('sort-content').clientHeight, 'Sort criteria must scroll');
        el('btn-close-sort').click();
        win.toolbarTest.emit('help', helpFixture);
        fits('help-modal');
        check(el('help-content').scrollHeight > el('help-content').clientHeight, 'Help must scroll');
        el('btn-close-help').click();
        win.toolbarTest.emit('match-results', matchFixture);
        fits('match-modal');
        check(el('match-content').scrollHeight > el('match-content').clientHeight, 'Match results must scroll');
        check(el('match-content').scrollWidth <= el('match-content').clientWidth + 1, 'Long filenames fit narrow viewport');
      } finally { frame.remove(); }
    }]
  ];
  async function run() {
    const results = [];
    for (const [name, test] of tests) {
      try { reset(); await test(); results.push({ name, passed: true }); }
      catch (error) { results.push({ name, passed: false, error: String(error) }); }
    }
    reset();
    window.toolbarTestResults = results;
    $('test-result').textContent = `${results.filter(result => result.passed).length}/${results.length} GUI tests passed`;
    $('test-result').title = results.filter(result => !result.passed).map(result => `${result.name}: ${result.error}`).join('\n');
  }
  toolbarTest.ready.then(async () => {
    const panel = document.createElement('div');
    panel.style.cssText = 'position:fixed;top:30px;left:12px;z-index:3000;background:#222;padding:8px;display:flex;gap:10px;font-size:12px;color:#eee;';
    panel.innerHTML = '<button id="run-toolbar-tests">Run GUI tests</button><button id="large-fixture">Large index</button><button id="long-fixture">Long filename</button><button id="help-fixture">Help popup</button><button id="match-fixture">Match popup</button><span id="test-result">Ready</span>';
    document.body.appendChild(panel);
    $('run-toolbar-tests').addEventListener('click', run);
    $('large-fixture').addEventListener('click', () => toolbarTest.setMeta({ current_index: 12345, total: 12480 }));
    $('long-fixture').addEventListener('click', () => {
      if (!$('overlay').classList.contains('full')) $('toolbar-toggle').click();
      toolbarTest.setMeta({ display_text: '图像 [final] '.repeat(100) + '.webp' });
    });
    $('help-fixture').addEventListener('click', () => toolbarTest.emit('help', helpFixture));
    $('match-fixture').addEventListener('click', () => toolbarTest.emit('match-results', matchFixture));
    await run();
  });
})();
