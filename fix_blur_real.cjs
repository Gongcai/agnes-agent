const fs = require('fs');
const path = require('path');

function replaceFile(file, regex, replacement) {
  const p = path.join(__dirname, file);
  if (fs.existsSync(p)) {
    let content = fs.readFileSync(p, 'utf-8');
    content = content.replace(regex, replacement);
    fs.writeFileSync(p, content, 'utf-8');
  }
}

// CommandPalette.tsx
replaceFile('src/components/CommandPalette.tsx',
  /className="flex items-center gap-3 px-4 py-3\.5 border-b border-hairline bg-canvas-surface"/g,
  'className="flex items-center gap-3 px-4 py-3.5 border-b border-hairline bg-transparent"');

// ui/modal.tsx
replaceFile('src/components/ui/modal.tsx',
  /bg-canvas-surface text-ink shadow-modal/g,
  'bg-canvas-surface backdrop-blur-xl text-ink shadow-modal');

// AppContextMenu.tsx
replaceFile('src/components/AppContextMenu.tsx',
  /bg-canvas-surface text-ink shadow-dropdown/g,
  'bg-canvas-surface backdrop-blur-xl text-ink shadow-dropdown');
