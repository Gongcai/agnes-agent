const fs = require('fs');
const path = require('path');

function replaceClass(file, target, replacement) {
  const p = path.join(__dirname, file);
  if (fs.existsSync(p)) {
    let content = fs.readFileSync(p, 'utf-8');
    content = content.replace(target, replacement);
    fs.writeFileSync(p, content, 'utf-8');
  }
}

// CommandPalette.tsx
replaceClass('src/components/CommandPalette.tsx',
  'bg-canvas-surface text-ink shadow-floating animate-scale-in',
  'bg-canvas-surface backdrop-blur-xl text-ink shadow-modal animate-scale-in');

// ui/modal.tsx
replaceClass('src/components/ui/modal.tsx',
  'relative w-full max-w-2xl transform overflow-hidden rounded-2xl bg-canvas-surface p-6 text-left align-middle shadow-modal transition-all',
  'relative w-full max-w-2xl transform overflow-hidden rounded-2xl bg-canvas-surface backdrop-blur-xl p-6 text-left align-middle shadow-modal transition-all');

// SettingsModal uses Modal, ConfirmDialog uses Modal, ModifyMemoryModal uses Modal, so ui/modal.tsx fixes them.

// AppContextMenu.tsx
replaceClass('src/components/AppContextMenu.tsx',
  'bg-canvas-surface text-ink shadow-dropdown',
  'bg-canvas-surface backdrop-blur-xl text-ink shadow-dropdown');

// AppTitleBar doesn't need blur (it's opaque or header) - wait, header is --claude-header which is already rgba(..., 0.96), with backdrop-filter.
// Let's check index.css to see if agnes-titlebar has backdrop-blur.
