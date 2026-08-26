const fs = require('fs');
const path = require('path');
const p = path.join(__dirname, 'src/components/PlannerWorkspace.tsx');
let content = fs.readFileSync(p, 'utf-8');
content = content.replace(/<CalendarBlankWorkspace/g, '<CalendarWorkspace');
fs.writeFileSync(p, content, 'utf-8');
