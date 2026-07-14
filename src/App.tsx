import { useEffect, useState } from "react";
import { Button } from "./components/ui/button";
import { ping, listAgents, type AgentSummary } from "./lib/ipc";

export default function App() {
  const [msg, setMsg] = useState<string>("");
  const [agents, setAgents] = useState<AgentSummary[]>([]);

  useEffect(() => {
    ping().then(setMsg).catch((e) => setMsg(String(e)));
    listAgents().then(setAgents).catch(() => setAgents([]));
  }, []);

  return (
    <div className="p-8 space-y-4">
      <h1 className="text-xl font-bold">agnes-agent</h1>
      <p className="text-sm text-zinc-600">Rust 回显：{msg}</p>
      <p className="text-sm text-zinc-600">Agent 数量：{agents.length}</p>
      <Button>开始</Button>
    </div>
  );
}
