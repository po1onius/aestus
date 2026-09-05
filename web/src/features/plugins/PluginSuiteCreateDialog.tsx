import { Loader2, Plus } from "lucide-react";
import { useState, type FormEvent } from "react";
import { Modal } from "../../components/Modal";
import { buttonPrimary, fieldHelp, fieldLabel, fieldStack, inputClass, spinnerClass, textareaClass } from "../../lib/ui";
import type { PluginSummary, UpstreamApiKeyProvider } from "../../types";
import { pluginSourceLabel } from "./access";
import { pluginSlotLabels, suiteSlotFields } from "./slots";

export interface CreatePluginSuiteInput {
  name: string;
  description: string;
  provider: UpstreamApiKeyProvider;
  request_plugin_id: string | null;
  buffered_response_plugin_id: string | null;
  stream_response_plugin_id: string | null;
}

interface Props {
  isPlatformAdmin: boolean;
  plugins: PluginSummary[];
  saving: boolean;
  onCreate: (input: CreatePluginSuiteInput) => Promise<boolean>;
  onClose: () => void;
}

export function PluginSuiteCreateDialog({ plugins, saving, onCreate, onClose, isPlatformAdmin }: Props) {
  const [input, setInput] = useState<CreatePluginSuiteInput>({
    name: "", description: "", provider: "gpt",
    request_plugin_id: null, buffered_response_plugin_id: null, stream_response_plugin_id: null,
  });
  const hasPlugin = suiteSlotFields.some(({ field }) => input[field]);

  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (saving || !input.name.trim() || !hasPlugin) return;
    if (await onCreate(input)) onClose();
  }

  return (
    <Modal titleId="pluginSuiteCreateTitle" title={isPlatformAdmin ? "创建平台公共套件" : "创建本租户套件"} description="从已有插件中组合套件。创建后不能更换插件搭配。" closeDisabled={saving} onClose={onClose}>
      <form className="grid gap-4" onSubmit={submit}>
        <label className={fieldStack}>
          <span className={fieldLabel}>套件名称</span>
          <input className={inputClass} required autoFocus disabled={saving} maxLength={128} value={input.name} onChange={(e) => setInput({ ...input, name: e.target.value })} />
        </label>
        <label className={fieldStack}>
          <span className={fieldLabel}>Provider</span>
          <select className={inputClass} disabled={saving} value={input.provider} onChange={(e) => setInput({ ...input, provider: e.target.value as UpstreamApiKeyProvider, request_plugin_id: null, buffered_response_plugin_id: null, stream_response_plugin_id: null })}>
            <option value="gpt">GPT · Responses</option>
            <option value="claude">Claude · Messages</option>
          </select>
        </label>
        {suiteSlotFields.map(({ slot, field }) => (
          <label className={fieldStack} key={slot}>
            <span className={fieldLabel}>{pluginSlotLabels[slot]}</span>
            <select className={inputClass} disabled={saving} value={input[field] ?? ""} onChange={(e) => setInput({ ...input, [field]: e.target.value || null })}>
              <option value="">不使用，沿用原生处理</option>
              {plugins.filter((p) => p.provider === input.provider && p.slot === slot && (!isPlatformAdmin || p.tenant_id === null)).map((p) => (
                <option key={p.id} value={p.id}>{p.name} · {pluginSourceLabel(p.tenant_id)}</option>
              ))}
            </select>
          </label>
        ))}
        <span className={fieldHelp}>至少选择一个插件。仅列出同 Provider、对应插槽的插件。</span>
        <label className={fieldStack}>
          <span className={fieldLabel}>备注</span>
          <textarea className={textareaClass} disabled={saving} maxLength={1024} value={input.description} onChange={(e) => setInput({ ...input, description: e.target.value })} />
        </label>
        <button className={`${buttonPrimary} w-full`} disabled={saving || !input.name.trim() || !hasPlugin}>
          {saving ? <Loader2 className={spinnerClass} size={18} /> : <Plus size={18} />}创建套件
        </button>
      </form>
    </Modal>
  );
}
