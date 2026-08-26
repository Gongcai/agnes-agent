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
  'Library as Books': 'Books',
  'FileText': 'FileText',
  'Image as ImageSquare': 'ImageSquare',
  'GitBranch': 'GitBranch',
  'Pencil': 'PencilSimple',
  'Puzzle as PuzzlePiece': 'PuzzlePiece',
  'RefreshCw': 'ArrowsClockwise',
  'ArrowUp as Send': 'ArrowUp as Send',
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
  'Home as House': 'House',
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
  'ListTodo': 'ListPlus',
  'type LucideIcon': 'type Icon'
};

function processFile(filePath) {
  let content = fs.readFileSync(filePath, 'utf-8');

  // Find import statement from lucide-react
  const regex = /import\s+{([^}]+)}\s+from\s+['"]lucide-react['"];?/g;

  content = content.replace(regex, (match, p1) => {
    let imports = p1.split(',').map(s => s.trim()).filter(s => s);

    let newImports = imports.map(imp => {
      let mapped = iconMapping[imp];
      if (mapped) {
        return mapped;
      }

      // Attempt fallback mapping
      if (imp.includes(' as ')) {
        const [original, alias] = imp.split(' as ').map(s => s.trim());
        const mappedOriginal = iconMapping[original] || original;
        return `${mappedOriginal} as ${alias}`;
      }

      console.log(`Unmapped icon in ${filePath}: ${imp}`);
      return imp;
    });

    // Deduplicate imports
    newImports = [...new Set(newImports)];

    return `import { ${newImports.join(', ')} } from "@phosphor-icons/react";`;
  });

  // Replace LucideIcon type with Icon
  content = content.replace(/LucideIcon/g, 'Icon');

  // Remove strokeWidth attributes
  content = content.replace(/\s+strokeWidth=\{[^}]+\}/g, '');
  content = content.replace(/\s+strokeWidth="[^"]+"/g, '');

  fs.writeFileSync(filePath, content, 'utf-8');
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
