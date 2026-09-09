import { AnimatePresence } from "motion/react";
import { useEffect, useState } from "react";
import { toast } from "sonner";
import { useRequestScope } from "../../api/useRequestScope";
import { dashboardListPageSize, maxUserConcurrency, maxUserQuota, usersPath } from "../../config";
import { UserGroupGrantsDialog } from "../../features/group-access/UserGroupGrantsDialog";
import { UserConcurrencyDialog, type UserConcurrencyDialogInput } from "../../features/users/UserConcurrencyDialog";
import { UserCreateDialog, type UserCreateDialogInput } from "../../features/users/UserCreateDialog";
import { UserQuotaDialog, type UserQuotaDialogInput } from "../../features/users/UserQuotaDialog";
import { showErrorToast } from "../../lib/errors";
import { initialListPageState, listPagePath, ListPageState, pageStateFrom } from "../../lib/pagination";
import { utf8ByteLength } from "../../lib/validation";
import { UsersPage } from "../../pages/UsersPage";
import type { DashboardUser, DashboardUserListItem, ListUsersResponse } from "../../types";
import { useProviderGroups } from "../accounts/useProviderGroups";
import { useDashboard, type PageProps } from "../dashboard/context";

export function UsersScreen({ refreshRevision, onLoadingChange }: PageProps) {
  const { authToken, currentUser, setCurrentUser } = useDashboard();
  const { providerGroups, loadProviderGroups } = useProviderGroups(authToken);
  const { isActiveAuthToken, requestJson, beginRequest } = useRequestScope(authToken);
  const [users, setUsers] = useState<DashboardUserListItem[]>([]);

  const [usersPage, setUsersPage] = useState<ListPageState>(initialListPageState);

  const [usersLoading, setUsersLoading] = useState(false);

  const [userUpdatingId, setUserUpdatingId] = useState<string | null>(null);

  const [userQuotaDialogUser, setUserQuotaDialogUser] = useState<DashboardUser | null>(null);

  const [userConcurrencyDialogUser, setUserConcurrencyDialogUser] =
    useState<DashboardUser | null>(null);

  const [userCreateOpen, setUserCreateOpen] = useState(false);

  const [userGroupGrantsDialogUser, setUserGroupGrantsDialogUser] =
    useState<DashboardUser | null>(null);

  const [userCreating, setUserCreating] = useState(false);

  async function loadUsers(offset = usersPage.offset) {
    const token = authToken;
    const request = beginRequest("loadUsers");
    if (!token) {
      setUsers([]);
      setUsersPage(initialListPageState());
      setUsersLoading(false);
      return;
    }

    setUsersLoading(true);
    try {
      const data = await requestJson<ListUsersResponse>(listPagePath(usersPath, offset), { signal: request.signal }, token);
      if (!request.isCurrent()) {
        return;
      }
      setUsers(data.items);
      setUsersPage(pageStateFrom(data));
    } catch (error) {
      if (request.isCurrent()) {
        showErrorToast("用户加载失败", error);
      }
    } finally {
      if (request.isCurrent()) {
        setUsersLoading(false);
      }
    }
  }

  function openUserQuotaDialog(user: DashboardUser) {
    setUserQuotaDialogUser(user);
  }

  function closeUserQuotaDialog() {
    if (userQuotaDialogUser && userUpdatingId === userQuotaDialogUser.id) {
      return;
    }
    setUserQuotaDialogUser(null);
  }

  function openUserConcurrencyDialog(user: DashboardUser) {
    setUserConcurrencyDialogUser(user);
  }

  function closeUserConcurrencyDialog() {
    if (userConcurrencyDialogUser && userUpdatingId === userConcurrencyDialogUser.id) {
      return;
    }
    setUserConcurrencyDialogUser(null);
  }

  function closeUserCreateDialog() {
    if (userCreating) {
      return;
    }
    setUserCreateOpen(false);
  }

  async function submitUserQuota({ value: userQuotaValue }: UserQuotaDialogInput) {
    if (!userQuotaDialogUser) {
      return;
    }

    const quota = Number(userQuotaValue.trim());
    if (!Number.isSafeInteger(quota) || quota < 0 || quota > maxUserQuota) {
      toast.error("用户额度更新失败", {
        description: `额度必须是 0 到 ${maxUserQuota} 之间的安全整数。`,
      });
      return;
    }

    setUserUpdatingId(userQuotaDialogUser.id);
    try {
      const updated = await requestJson<DashboardUser>(`${usersPath}/${userQuotaDialogUser.id}/quota`, {
        method: "PUT",
        body: JSON.stringify({ quota }),
      }, authToken);
      setUsers((items) =>
        items.map((item) => (item.id === updated.id ? { ...item, ...updated } : item)),
      );
      if (currentUser?.id === updated.id) {
        setCurrentUser(updated);
      }
      setUserQuotaDialogUser(null);
      if (isActiveAuthToken(authToken)) toast.success("用户额度已更新");
    } catch (error) {
      showErrorToast("用户额度更新失败", error);
    } finally {
      setUserUpdatingId(null);
    }
  }

  async function submitUserConcurrency({ value: userConcurrencyValue }: UserConcurrencyDialogInput) {
    if (!userConcurrencyDialogUser) {
      return;
    }

    const normalizedValue = userConcurrencyValue.trim();
    const maxConcurrency = normalizedValue === "" ? null : Number(normalizedValue);
    if (
      maxConcurrency !== null &&
      (!Number.isSafeInteger(maxConcurrency) || maxConcurrency < 1 || maxConcurrency > maxUserConcurrency)
    ) {
      toast.error("用户并发上限更新失败", {
        description: `并发上限必须留空，或填写 1 到 ${maxUserConcurrency} 之间的整数。`,
      });
      return;
    }

    setUserUpdatingId(userConcurrencyDialogUser.id);
    try {
      const updated = await requestJson<DashboardUser>(
        `${usersPath}/${userConcurrencyDialogUser.id}/max-concurrency`,
        {
          method: "PUT",
          body: JSON.stringify({ max_concurrency: maxConcurrency }),
        },
        authToken,
      );
      setUsers((items) =>
        items.map((item) => (item.id === updated.id ? { ...item, ...updated } : item)),
      );
      if (currentUser?.id === updated.id) {
        setCurrentUser(updated);
      }
      setUserConcurrencyDialogUser(null);
      if (isActiveAuthToken(authToken)) toast.success("用户并发上限已更新");
    } catch (error) {
      showErrorToast("用户并发上限更新失败", error);
    } finally {
      setUserUpdatingId(null);
    }
  }

  async function submitUserCreate({ username: userCreateUsername, email: userCreateEmail, password: userCreatePassword }: UserCreateDialogInput) {
    const username = userCreateUsername.trim().toLowerCase();
    const usernameCharacters = Array.from(username);
    if (
      usernameCharacters.length < 5 ||
      usernameCharacters.length > 32 ||
      utf8ByteLength(username) > 128 ||
      !/^[\p{L}\p{N}][\p{L}\p{N}_-]*$/u.test(username)
    ) {
      toast.error("用户添加失败", {
        description: "用户名必须为 5 到 32 个字符，只能包含字母、数字、下划线和连字符。",
      });
      return;
    }
    if (Array.from(userCreatePassword).length < 8 || utf8ByteLength(userCreatePassword) > 72) {
      toast.error("用户添加失败", {
        description: "密码至少 8 个字符，且 UTF-8 编码长度不能超过 72 字节。",
      });
      return;
    }

    const email = userCreateEmail.trim();
    setUserCreating(true);
    try {
      await requestJson<DashboardUser>(usersPath, {
        method: "POST",
        // 邮箱留空时不发送该字段，确保默认地址规则由服务端作为唯一真源执行。
        body: JSON.stringify({
          username,
          ...(email ? { email } : {}),
          password: userCreatePassword,
        }),
      }, authToken);
      setUserCreateOpen(false);
      await loadUsers(0);
      if (isActiveAuthToken(authToken)) toast.success("用户已添加");
    } catch (error) {
      showErrorToast("用户添加失败", error);
    } finally {
      setUserCreating(false);
    }
  }

  async function updateUserStatus(user: DashboardUser) {
    setUserUpdatingId(user.id);
    try {
      const updated = await requestJson<DashboardUser>(`${usersPath}/${user.id}/status`, {
        method: "PUT",
        body: JSON.stringify({ enabled: !user.enabled }),
      }, authToken);
      setUsers((items) =>
        items.map((item) => (item.id === updated.id ? { ...item, ...updated } : item)),
      );
      if (isActiveAuthToken(authToken)) toast.success(updated.enabled ? "用户已启用" : "用户已禁用");
    } catch (error) {
      showErrorToast("用户状态更新失败", error);
    } finally {
      setUserUpdatingId(null);
    }
  }
  useEffect(() => { void loadUsers(); void loadProviderGroups(); }, [refreshRevision]);
  useEffect(() => { onLoadingChange(usersLoading); return () => onLoadingChange(false); }, [usersLoading, onLoadingChange]);
  return <><UsersPage
    users={users}
    loading={usersLoading}
    updatingId={userUpdatingId}
    currentUserId={currentUser.id}
    offset={usersPage.offset}
    pageSize={dashboardListPageSize}
    nextOffset={usersPage.nextOffset}
    onAdd={() => setUserCreateOpen(true)}
    onOpenQuota={openUserQuotaDialog}
    onOpenConcurrency={openUserConcurrencyDialog}
    onOpenGroupGrants={setUserGroupGrantsDialogUser}
    onToggleStatus={updateUserStatus}
    onPageChange={loadUsers}
  /><AnimatePresence>{userQuotaDialogUser && (
    <UserQuotaDialog
      key={`user-quota-${userQuotaDialogUser.id}`}
      user={userQuotaDialogUser}
      saving={userUpdatingId === userQuotaDialogUser.id}
      onSubmit={submitUserQuota}
      onClose={closeUserQuotaDialog}
    />
  )}
      {userConcurrencyDialogUser && (
        <UserConcurrencyDialog
          key={`user-concurrency-${userConcurrencyDialogUser.id}`}
          user={userConcurrencyDialogUser}
          maxValue={maxUserConcurrency}
          saving={userUpdatingId === userConcurrencyDialogUser.id}
          onSubmit={submitUserConcurrency}
          onClose={closeUserConcurrencyDialog}
        />
      )}
      {userCreateOpen && (
        <UserCreateDialog
          key="user-create"
          saving={userCreating}
          onSubmit={submitUserCreate}
          onClose={closeUserCreateDialog}
        />
      )}
      {userGroupGrantsDialogUser && authToken && (
        <UserGroupGrantsDialog
          key={`user-group-grants-${userGroupGrantsDialogUser.id}`}
          user={userGroupGrantsDialogUser}
          groups={providerGroups}
          token={authToken}
          onClose={() => setUserGroupGrantsDialogUser(null)}
        />
      )}</AnimatePresence></>;
}
