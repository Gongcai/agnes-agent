import type { ModelProvider } from "../store/useAgentStore";

export function filterModelProviders(providers: ModelProvider[], query: string, providerId = "") {
  const terms = query.normalize("NFC").trim().toLocaleLowerCase().split(/\s+/).filter(Boolean);
  return providers
    .filter((provider) => !providerId || provider.id === providerId)
    .map((provider) => ({
      ...provider,
      models: provider.models.filter((model) => {
        const text = `${provider.name} ${model.id}`.normalize("NFC").toLocaleLowerCase();
        return terms.every((term) => text.includes(term));
      }),
    }))
    .filter((provider) => provider.models.length > 0);
}
