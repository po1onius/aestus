import { Loader2, Save } from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import { toast } from "sonner";
import { requestJson } from "../../api/client";
import { Modal } from "../../components/Modal";
import { usersPath } from "../../config";
import { showErrorToast } from "../../lib/errors";
import {
  buttonPrimary,
  emptyStateClass,
  spinnerClass,
} from "../../lib/ui";
import type {
  DashboardUser,
  GroupPermission,
  ProviderGroupSummary,
  UserGroupGrant,
} from "../../types";

interface Props {
  user: DashboardUser;
  groups: ProviderGroupSummary[];
  token: string;
  onClose: () => void;
}

const permissionOptions: Array<{
  permission: GroupPermission;
  label: string;
  detail: string;
  requires: GroupPermission[];
}> = [
  { permission: "account.view", label: "查看账号", detail: "查看组内 OAuth 账号及运行状态", requires: [] },
  { permission: "account.quota.view", label: "查看账号额度", detail: "主动查询 GPT 账号额度", requires: ["account.view"] },
  { permission: "account.reset.view", label: "查看重置信息", detail: "查看 GPT 可用重置次数和记录", requires: ["account.view"] },
  { permission: "account.reset.consume", label: "使用重置次数", detail: "应用重置并同步账号额度", requires: ["account.view", "account.quota.view", "account.reset.view"] },
  { permission: "account.override.view", label: "查看账号覆盖", detail: "读取账号 Header/Body 覆盖", requires: ["account.view"] },
  { permission: "account.override.update", label: "修改账号覆盖", detail: "编辑账号 Header/Body 覆盖", requires: ["account.view", "account.override.view"] },
  { permission: "official_api_key.view", label: "查看官方 Key", detail: "查看组内脱敏后的官方 API Key", requires: [] },
  { permission: "official_api_key.override.view", label: "查看官方 Key 覆盖", detail: "读取官方 Key 请求覆盖", requires: ["official_api_key.view"] },
  { permission: "official_api_key.override.update", label: "修改官方 Key 覆盖", detail: "编辑官方 Key 请求覆盖", requires: ["official_api_key.view", "official_api_key.override.view"] },
];

export function UserGroupGrantsDialog({ user, groups, token, onClose }: Props) {
  const [grants, setGrants] = useState<Record<string, Set<GroupPermission>>>({});
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const sortedGroups = useMemo(
    () => [...groups].sort((a, b) => a.provider.localeCompare(b.provider) || a.name.localeCompare(b.name)),
    [groups],
  );

  useEffect(() => {
    let active = true;
    setLoading(true);
    requestJson<UserGroupGrant[]>(`${usersPath}/${user.id}/group-grants`, undefined, token)
      .then((items) => {
        if (active) {
          setGrants(Object.fromEntries(items.map((item) => [item.group_id, new Set(item.permissions)])));
        }
      })
      .catch((error) => active && showErrorToast("分组授权加载失败", error))
      .finally(() => active && setLoading(false));
    return () => {
      active = false;
    };
  }, [token, user.id]);

  function toggleGroup(groupId: string, enabled: boolean) {
    setGrants((current) => {
      const next = { ...current };
      if (enabled) {
        next[groupId] = new Set();
      } else {
        delete next[groupId];
      }
      return next;
    });
  }

  function togglePermission(groupId: string, permission: GroupPermission, enabled: boolean) {
    setGrants((current) => {
      const selected = new Set(current[groupId] ?? []);
      if (enabled) {
        addWithPrerequisites(selected, permission);
      } else {
        selected.delete(permission);
        for (const option of permissionOptions) {
          if (dependsOn(option.permission, permission)) {
            selected.delete(option.permission);
          }
        }
      }
      return { ...current, [groupId]: selected };
    });
  }

  async function save() {
    setSaving(true);
    try {
      const payload = Object.entries(grants).map(([groupId, permissions]) => ({
        group_id: groupId,
        permissions: [...permissions].sort(),
      }));
      const saved = await requestJson<UserGroupGrant[]>(
        `${usersPath}/${user.id}/group-grants`,
        { method: "PUT", body: JSON.stringify({ grants: payload }) },
        token,
      );
      setGrants(Object.fromEntries(saved.map((item) => [item.group_id, new Set(item.permissions)])));
      toast.success("分组授权已更新");
      onClose();
    } catch (error) {
      showErrorToast("分组授权更新失败", error);
    } finally {
      setSaving(false);
    }
  }

  return (
    <Modal
      titleId="userGroupGrantsTitle"
      title="分组授权"
      description={`用户：${user.username}`}
      className="max-w-5xl"
      closeDisabled={saving}
      onClose={onClose}
    >
      {loading ? (
        <div className={emptyStateClass}>
          <Loader2 className={spinnerClass} size={22} />
          正在加载授权
        </div>
      ) : (
        <div className="grid gap-4">
          <p className="text-sm leading-6 text-slate-600 dark:text-slate-300">
            开启分组授权后，用户可以用该分组创建和运行自己的网关 API Key。资源权限不会自动开启。
          </p>
          <div className="grid max-h-[65vh] gap-4 overflow-y-auto pr-1">
            {sortedGroups.map((group) => {
              const selected = grants[group.id];
              return (
                <section key={group.id} className="rounded-xl border border-slate-200 p-4 dark:border-slate-800">
                  <label className="flex cursor-pointer items-start gap-3">
                    <input
                      type="checkbox"
                      className="mt-1 accent-indigo-600"
                      checked={Boolean(selected)}
                      disabled={saving}
                      onChange={(event) => toggleGroup(group.id, event.target.checked)}
                    />
                    <span>
                      <strong className="text-sm text-slate-900 dark:text-slate-100">
                        {group.provider.toUpperCase()} / {group.name}
                      </strong>
                      <span className="ml-2 text-xs text-slate-500">
                        {group.enabled ? "启用" : "已停用"}
                      </span>
                    </span>
                  </label>
                  {selected && (
                    <div className="mt-4 grid gap-2 border-t border-slate-100 pt-4 dark:border-slate-800 sm:grid-cols-2 lg:grid-cols-3">
                      {permissionOptions.map((option) => (
                        <label key={option.permission} className="flex cursor-pointer items-start gap-2 rounded-lg p-2 hover:bg-slate-50 dark:hover:bg-slate-800/60">
                          <input
                            type="checkbox"
                            className="mt-1 accent-indigo-600"
                            checked={selected.has(option.permission)}
                            disabled={saving}
                            onChange={(event) => togglePermission(group.id, option.permission, event.target.checked)}
                          />
                          <span>
                            <span className="block text-sm font-medium text-slate-800 dark:text-slate-200">{option.label}</span>
                            <span className="block text-xs leading-5 text-slate-500 dark:text-slate-400">{option.detail}</span>
                          </span>
                        </label>
                      ))}
                    </div>
                  )}
                </section>
              );
            })}
          </div>
          <div className="flex justify-end">
            <button type="button" className={buttonPrimary} disabled={saving} onClick={save}>
              {saving ? <Loader2 className={spinnerClass} size={18} /> : <Save size={18} />}
              保存授权
            </button>
          </div>
        </div>
      )}
    </Modal>
  );
}

function addWithPrerequisites(selected: Set<GroupPermission>, permission: GroupPermission) {
  selected.add(permission);
  const option = permissionOptions.find((candidate) => candidate.permission === permission);
  for (const required of option?.requires ?? []) {
    addWithPrerequisites(selected, required);
  }
}

function dependsOn(permission: GroupPermission, possibleParent: GroupPermission): boolean {
  const option = permissionOptions.find((candidate) => candidate.permission === permission);
  return (option?.requires ?? []).some(
    (required) => required === possibleParent || dependsOn(required, possibleParent),
  );
}
