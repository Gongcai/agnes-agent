const fs = require('fs');
const path = require('path');

const iconMapping = {
  'Plus': 'Plus',
  'Code2': 'BracketsCurly',
  'Maximize2': 'ArrowsOut',
  'Minimize2': 'ArrowsIn',
  'Minus': 'Minus',
  'PanelLeft': 'SidebarSimple',
  'Search': 'MagnifyingGlass',
  'X': 'X',
  'AlertTriangle': 'Warning',
  'Brain': 'Brain',
  'Check': 'Check',
  'ChevronDown': 'CaretDown',
  'Copy': 'Copy',
  'Cpu': 'Cpu',
  'FileText': 'FileText',
  'GitBranch': 'GitBranch',
  'Pencil': 'PencilSimple',
  'RefreshCw': 'ArrowsClockwise',
  'Server': 'HardDrives',
  'ShieldCheck': 'ShieldCheck',
  'Sparkles': 'Sparkle',
  'Square': 'Square',
  'Terminal': 'Terminal',
  'Trash2': 'Trash',
  'BookOpen': 'BookOpen',
  'Calendar': 'CalendarBlank',
  'CheckSquare': 'CheckSquare',
  'Database': 'Database',
  'HardDrive': 'HardDrive',
  'MessageSquare': 'Chat',
  'Moon': 'Moon',
  'Settings': 'Gear',
  'Sun': 'Sun',
  'AlertCircle': 'WarningCircle',
  'ArrowDown': 'ArrowDown',
  'ArrowUp': 'ArrowUp',
  'ChevronRight': 'CaretRight',
  'Cloud': 'Cloud',
  'Download': 'DownloadSimple',
  'Eye': 'Eye',
  'EyeOff': 'EyeSlash',
  'File': 'File',
  'Folder': 'Folder',
  'FolderDown': 'FolderMinus',
  'FolderInput': 'FolderPlus',
  'FolderOpen': 'FolderOpen',
  'LoaderCircle': 'CircleNotch',
  'MoreHorizontal': 'DotsThree',
  'Play': 'Play',
  'ShieldAlert': 'ShieldWarning',
  'Upload': 'UploadSimple',
  'ChevronsUpDown': 'ArrowsDownUp',
  'CloudUpload': 'CloudArrowUp',
  'FilePlus2': 'FilePlus',
  'Files': 'Files',
  'FolderPlus': 'FolderPlus',
  'MonitorSmartphone': 'Devices',
  'ArrowLeft': 'ArrowLeft',
  'BookMarked': 'BookmarkSimple',
  'ChevronLeft': 'CaretLeft',
  'FileUp': 'FileArrowUp',
  'Highlighter': 'HighlighterCircle',
  'History': 'ClockCounterClockwise',
  'Languages': 'Translate',
  'MessageCircleMore': 'ChatCircleDots',
  'NotebookPen': 'Notepad',
  'PanelRightClose': 'SidebarSimple',
  'PanelRightOpen': 'SidebarSimple',
  'Quote': 'Quotes',
  'RotateCcw': 'ArrowCounterClockwise',
  'Send': 'PaperPlaneRight',
  'Settings2': 'Faders',
  'SlidersHorizontal': 'SlidersHorizontal',
  'Type': 'TextT',
  'User': 'User',
  'CircleUserRound': 'UserCircle',
  'Camera': 'Camera',
  'Sliders': 'Sliders',
  'ShieldOff': 'ShieldSlash',
  'Key': 'Key',
  'Zap': 'Lightning',
  'GitCompareArrows': 'GitDiff',
  'Laptop': 'Laptop',
  'LockKeyhole': 'LockKey',
  'FileKey2': 'FileLock',
  'Globe2': 'GlobeHemisphereWest',
  'BarChart3': 'ChartBar',
  'Monitor': 'Monitor',
  'Eraser': 'Eraser',
  'Gauge': 'Gauge',
  'Puzzle': 'PuzzlePiece',
  'ChevronUp': 'CaretUp',
  'Pin': 'PushPin',
  'PinOff': 'PushPinSlash',
  'ExternalLink': 'ArrowSquareOut',
  'CircleStop': 'StopCircle',
  'Lightbulb': 'Lightbulb',
  'MessageSquareText': 'ChatText',
  'CalendarDays': 'Calendar',
  'CheckSquare2': 'CheckSquare',
  'CirclePlus': 'PlusCircle',
  'CalendarPlus2': 'CalendarPlus',
  'Clock3': 'Clock',
  'Repeat2': 'Repeat',
  'CalendarClock': 'Calendar',
  'Star': 'Star',
  'Circle': 'Circle',
  'CheckCircle2': 'CheckCircle',
  'Inbox': 'Tray',
  'ListChecks': 'ListChecks',
  'ListTodo': 'ListPlus'
};

function processFile(filePath) {
  let content = fs.readFileSync(filePath, 'utf-8');
  let changed = false;

  for (const [oldName, newName] of Object.entries(iconMapping)) {
    if (oldName === newName) continue;

    // Replace JSX opening tag <OldName ...> or <OldName>
    const jsxRegex = new RegExp(`<${oldName}(\\s|>)`, 'g');
    if (jsxRegex.test(content)) {
      content = content.replace(jsxRegex, `<${newName}$1`);
      changed = true;
    }

    // Replace icon reference when passed as a prop, e.g., icon={OldName}
    const propRegex = new RegExp(`({|\\s)${oldName}(}|,|\\s)`, 'g');
    if (propRegex.test(content)) {
      content = content.replace(propRegex, `$1${newName}$2`);
      changed = true;
    }
  }

  if (changed) {
    fs.writeFileSync(filePath, content, 'utf-8');
  }
}

function processDir(dir) {
  const files = fs.readdirSync(dir);
  for (const file of files) {
    const fullPath = path.join(dir, file);
    if (fs.statSync(fullPath).isDirectory()) {
      processDir(fullPath);
    } else if (fullPath.endsWith('.tsx') || fullPath.endsWith('.ts')) {
      processFile(fullPath);
    }
  }
}

processDir('src');
