const fs = require('fs');
const path = require('path');
const p = path.join(__dirname, 'src/index.css');
let content = fs.readFileSync(p, 'utf-8');
content = content.replace(
  /\.agnes-context-menu {\n  background: var\(--claude-surface\);/,
  '.agnes-context-menu {\n  background: var(--claude-surface);\n  backdrop-filter: blur(24px);\n  -webkit-backdrop-filter: blur(24px);'
);
fs.writeFileSync(p, content, 'utf-8');
