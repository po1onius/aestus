import { Loader2, Save } from "lucide-react";
import { useState } from "react";
import { toast } from "sonner";
import { isProviderStateSyncError } from "../../api/client";
import { useRequestScope } from "../../api/useRequestScope";
import { Modal } from "../../components/Modal";
import { gptAccountsPath } from "../../config";
import { showErrorToast } from "../../lib/errors";
import { buttonPrimary, fieldHelp, fieldLabel, fieldStack, inputClass, spinnerClass } from "../../lib/ui";
import type { GptAccount } from "../../types";

interface Props {
  account: GptAccount;
  authToken: string;
  onClose: () => void;
  onSaved: () => void;
}

export function GptAccountProxyDialog({ account, authToken, onClose, onSaved }: Props) {
  const [enabled, setEnabled] = useState(account.proxy !== null);
  const [url, setUrl] = useState(account.proxy?.url ?? "");
  const [keepAuth, setKeepAuth] = useState(account.proxy?.has_auth ?? false);
  const [saving, setSaving] = useState(false);
  const { requestJson, isActiveAuthToken } = useRequestScope(authToken);

  async function save() {
    if (enabled && !url.trim()) {
      toast.error("请填写代理 URL");
      return;
    }
    setSaving(true);
    try {
      await requestJson<GptAccount>(`${gptAccountsPath}/${account.id}/proxy`, {
        method: "PUT",
        body: JSON.stringify({ proxy_url: enabled ? url.trim() : null, keep_auth: enabled && keepAuth }),
      });
      toast.success("账号代理已保存");
      onSaved();
    } catch (error) {
      if (!isActiveAuthToken(authToken)) return;
      showErrorToast("代理保存失败", error);
      if (isProviderStateSyncError(error)) onSaved();
    } finally {
      if (isActiveAuthToken(authToken)) setSaving(false);
    }
  }

  return (
    <Modal titleId="gptAccountProxyTitle" title="代理设置"
      description={account.email || account.id} closeDisabled={saving} onClose={onClose}>
      <form className="grid gap-5" onSubmit={event => { event.preventDefault(); void save(); }}>
        <fieldset disabled={saving} className="flex gap-5">
          <legend className={`${fieldLabel} mb-2`}>连接方式</legend>
          <label className="flex items-center gap-2"><input type="radio" name="proxyMode" checked={!enabled} onChange={() => setEnabled(false)} />直连</label>
          <label className="flex items-center gap-2"><input type="radio" name="proxyMode" checked={enabled} onChange={() => setEnabled(true)} />指定代理</label>
        </fieldset>
        {enabled && (
          <div className={fieldStack}>
            <label className={fieldLabel} htmlFor="accountProxyUrl">代理 URL</label>
            <input id="accountProxyUrl" className={inputClass} value={url}
              onChange={event => setUrl(event.target.value)} required maxLength={4096}
              disabled={saving} autoComplete="off" spellCheck={false}
              placeholder="http://user:password@proxy.example:8080" />
            <p className={fieldHelp}>支持 HTTP、HTTPS、SOCKS5、SOCKS5H。需要认证时，可在 URL 中填写用户名和密码。</p>
            {account.proxy?.has_auth && (
              <>
                <label className="flex items-center gap-2 text-sm">
                  <input type="checkbox" checked={keepAuth} disabled={saving} onChange={event => setKeepAuth(event.target.checked)} />保留已保存的代理认证
                </label>
                <p className={fieldHelp}>{keepAuth
                  ? "现有认证信息不会显示；此时 URL 请勿包含用户名或密码。"
                  : "填写新的认证信息；URL 不含认证信息则清除原认证。"}</p>
              </>
            )}
          </div>
        )}
        <p className={fieldHelp}>应用于该账号的模型请求、额度接口和 token 刷新。代理不可用时不会自动改为直连。</p>
        <button type="submit" className={buttonPrimary} disabled={saving}>
          {saving ? <Loader2 size={18} className={spinnerClass} /> : <Save size={18} />}保存
        </button>
      </form>
    </Modal>
  );
}
