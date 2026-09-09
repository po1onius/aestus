import { claudeUpstreamApiKeysPath, gptUpstreamApiKeysPath } from "../../config";
import type { AccountProviderKey, UpstreamApiKeyProvider } from "../../types";
export function asUpstreamApiKeyProvider(
  provider: AccountProviderKey,
): UpstreamApiKeyProvider | null {
  return provider === "gpt" || provider === "claude" ? provider : null;
}

export function upstreamApiKeyPath(provider: UpstreamApiKeyProvider) {
  return provider === "claude" ? claudeUpstreamApiKeysPath : gptUpstreamApiKeysPath;
}

export function defaultUpstreamApiKeyBaseUrl(provider: UpstreamApiKeyProvider) {
  return provider === "claude" ? "https://api.anthropic.com" : "https://api.openai.com/v1";
}

export function providerLabel(provider: UpstreamApiKeyProvider) {
  return provider === "claude" ? "Claude" : "GPT";
}

export interface AccountPageOffsets {
  gpt: number;
  claude: number;
  gptUpstreamKeys: number;
  claudeUpstreamKeys: number;
}
