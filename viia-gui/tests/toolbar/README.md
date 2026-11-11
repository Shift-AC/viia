# Toolbar regression tests

From the repository root, run `python -m http.server 8766 --bind 127.0.0.1`.
Open `http://127.0.0.1:8766/viia-gui/tests/toolbar/index.html`.

The harness loads the production HTML directly and replaces only the Tauri transport.
Eighteen isolated GUI tests run automatically and can be repeated with **Run GUI tests**.
Results are available in `window.toolbarTestResults`, including failure messages.
The fixture buttons also support manual checks of large indices, long filenames, help, and matches.

These browser tests cover toolbar state, sizing, placement, accessibility key handling,
empty startup with delayed/missing metadata, local index spacing, toolbar mode preservation,
empty-state access, compact window-control placement and actions, popup submission/dismissal, long content in a 320×260 viewport,
and the existing terminal command bindings. They do not replace
native WebView2 validation or Rust backend tests. Nothing here is part of the Cargo workspace
or bundled frontend.
