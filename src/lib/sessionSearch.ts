interface SearchableSession {
  agent_id: string;
  title: string;
}

export function searchSessionsByTitle<T extends SearchableSession>(
  sessions: readonly T[],
  agentId: string | null,
  query: string,
): T[] {
  if (!agentId) return [];
  const normalizedQuery = query.trim().toLocaleLowerCase("zh-CN");
  return sessions.filter((session) => (
    session.agent_id === agentId
    && (!normalizedQuery || session.title.toLocaleLowerCase("zh-CN").includes(normalizedQuery))
  ));
}
