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

// NotificationCenter.tsx
replaceFile('src/components/NotificationCenter.tsx', /ShieldAlert/g, 'ShieldWarning');
replaceFile('src/components/NotificationCenter.tsx', /CheckSquare2/g, 'CheckSquare');
replaceFile('src/components/NotificationCenter.tsx', /CalendarDays/g, 'CalendarBlank');
replaceFile('src/components/NotificationCenter.tsx', /CheckCircle2/g, 'CheckCircle');
replaceFile('src/components/NotificationCenter.tsx', /CalendarDots as Calendar/g, 'CalendarBlank as CalendarBlank');
replaceFile('src/components/NotificationCenter.tsx', /CheckCircle as CheckCircle/g, 'CheckCircle');
replaceFile('src/components/NotificationCenter.tsx', /CheckSquare as CheckSquare/g, 'CheckSquare');
replaceFile('src/components/NotificationCenter.tsx', /ShieldWarning as ShieldWarning/g, 'ShieldWarning');

// PlannerWorkspace.tsx
replaceFile('src/components/PlannerWorkspace.tsx', /const HeaderIcon = isCalendar \? Calendar : CheckSquare2;/g, 'const HeaderIcon = isCalendar ? CalendarBlank : CheckSquare;');
replaceFile('src/components/PlannerWorkspace.tsx', /<Calendar/g, '<CalendarBlank');

// Sidebar.tsx
replaceFile('src/components/Sidebar.tsx', /calendar: Calendar,/g, 'calendar: CalendarBlank,');

// CalendarFilters.tsx
replaceFile('src/components/planner/calendar/CalendarFilters.tsx', /<Calendar\s/g, '<CalendarBlank ');

// DayAgenda.tsx
replaceFile('src/components/planner/calendar/DayAgenda.tsx', /<Calendar\s/g, '<CalendarBlank ');

// EventEditorDialog.tsx
replaceFile('src/components/planner/calendar/EventEditorDialog.tsx', /<Calendar\s/g, '<CalendarBlank ');

// QuickAdd.tsx
replaceFile('src/components/planner/tasks/QuickAdd.tsx', /<Calendar\s/g, '<CalendarBlank ');

// TaskDetailsDrawer.tsx
replaceFile('src/components/planner/tasks/TaskDetailsDrawer.tsx', /<Calendar\s/g, '<CalendarBlank ');

// TodoWorkspace.tsx
replaceFile('src/components/planner/tasks/TodoWorkspace.tsx', /icon: Calendar,/g, 'icon: CalendarBlank,');
replaceFile('src/components/planner/tasks/TodoWorkspace.tsx', /icon: Calendar }/g, 'icon: CalendarBlank }');
