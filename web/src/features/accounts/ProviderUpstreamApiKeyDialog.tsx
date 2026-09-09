import { Loader2, Save } from "lucide-react";
import { useState } from "react";
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

export interface ProviderUpstreamApiKeyDialogInput {
  apiKey: string;
  baseUrl: string;
}

interface ProviderUpstreamApiKeyDialogProps {
  providerLabel: string;
  baseUrlPlaceholder: string;
  saving: boolean;
  onSubmit: (input: ProviderUpstreamApiKeyDialogInput) => void;
  onClose: () => void;
}

/** GPT 与 Claude 共用的官方 API Key 导入边界，provider 只决定文案和默认 Base URL。 */
export function ProviderUpstreamApiKeyDialog(props: ProviderUpstreamApiKeyDialogProps) {
  const [apiKey, setApiKey] = useState<string>(() => "");
  const [baseUrl, setBaseUrl] = useState<string>(() => props.baseUrlPlaceholder);
  return (
    <Modal
      titleId="providerOfficialKeyTitle"
      title={`添加 ${props.providerLabel} 官方 Key`}
      closeDisabled={props.saving}
      onClose={props.onClose}
    >
      <div>
        <form className="grid gap-4" onSubmit={event => { event.preventDefault(); props.onSubmit({ apiKey, baseUrl }); }}>
          <label className={fieldStack}>
            <span className={fieldLabel}>
              API Key<span className={requiredMark}>*</span>
            </span>
            <textarea
              className={textareaClass}
              value={apiKey}
              onChange={(event) => setApiKey(event.target.value)}
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
              value={baseUrl}
              onChange={(event) => setBaseUrl(event.target.value)}
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
              apiKey.trim().length === 0 ||
              baseUrl.trim().length === 0
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
