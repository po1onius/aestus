import { Loader2, Save } from "lucide-react";
import type { FormEvent } from "react";
import { Modal } from "../../components/Modal";
import {
  buttonPrimary,
  fieldLabel,
  fieldStack,
  inputClass,
  requiredMark,
  spinnerClass,
  textareaClass,
} from "../../lib/ui";

interface ProviderUpstreamApiKeyDialogProps {
  providerLabel: string;
  apiKey: string;
  baseUrl: string;
  baseUrlPlaceholder: string;
  saving: boolean;
  onApiKeyChange: (value: string) => void;
  onBaseUrlChange: (value: string) => void;
  onSubmit: (event: FormEvent<HTMLFormElement>) => void;
  onClose: () => void;
}

/** GPT 与 Claude 共用的官方 API Key 导入边界，provider 只决定文案和默认 Base URL。 */
export function ProviderUpstreamApiKeyDialog(props: ProviderUpstreamApiKeyDialogProps) {
  return (
    <Modal
      titleId="providerOfficialKeyTitle"
      title={`添加 ${props.providerLabel} 官方 Key`}
      description="API Key 与 Base URL 导入后固定；需要更换时请删除后重新导入。"
      closeDisabled={props.saving}
      onClose={props.onClose}
    >
      <div>
        <form className="grid gap-4" onSubmit={props.onSubmit}>
          <p className="text-sm text-slate-500 dark:text-slate-400">
            官方 Key 会先以未分组状态保存；分配到分组前不会进入可调度池。
          </p>
          <label className={fieldStack}>
            <span className={fieldLabel}>
              API Key<span className={requiredMark}>*</span>
            </span>
            <textarea
              className={textareaClass}
              value={props.apiKey}
              onChange={(event) => props.onApiKeyChange(event.target.value)}
              rows={4}
              placeholder={`粘贴 ${props.providerLabel} 官方 API Key`}
              maxLength={4 * 1024}
              required
            />
          </label>
          <label className={fieldStack}>
            <span className={fieldLabel}>
              Base URL<span className={requiredMark}>*</span>
            </span>
            <input
              className={inputClass}
              value={props.baseUrl}
              onChange={(event) => props.onBaseUrlChange(event.target.value)}
              placeholder={props.baseUrlPlaceholder}
              autoComplete="off"
              maxLength={2 * 1024}
              required
            />
          </label>
          <button
            className={`${buttonPrimary} mt-1 w-full`}
            disabled={
              props.saving ||
              props.apiKey.trim().length === 0 ||
              props.baseUrl.trim().length === 0
            }
          >
            {props.saving ? <Loader2 className={spinnerClass} size={18} /> : <Save size={18} />}
            保存官方 Key
          </button>
        </form>
      </div>
    </Modal>
  );
}
