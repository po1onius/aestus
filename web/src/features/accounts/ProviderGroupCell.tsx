import { ProviderGroupPicker } from "../../components/ProviderGroupPicker";
import type { ProviderGroupReference, UpstreamApiKeyProvider } from "../../types";

interface ProviderGroupCellProps {
  resourceLabel: string;
  provider: UpstreamApiKeyProvider;
  group: ProviderGroupReference | null;
  groups: ProviderGroupReference[];
  disabled: boolean;
  onChange: (groupId: string) => void;
}

/**
 * 上游资源共用的所在组单元格。
 * 共享动画下拉框直接显示当前组名并承担迁移操作；同时补入当前组，确保选项数据短暂刷新时
 * 仍能正确展示。
 */
export function ProviderGroupCell({
  resourceLabel,
  provider,
  group,
  groups,
  disabled,
  onChange,
}: ProviderGroupCellProps) {
  const unassignedOption: ProviderGroupReference = {
    id: "",
    provider,
    name: "未分组",
    enabled: true,
    created_at: "",
    updated_at: "",
    disabled_at: null,
  };
  // 当前分组可能已停用，不会出现在启用分组选项中；仍需补入以正确展示，并始终提供
  // “未分组”选项，使资源可以主动退出调度边界。
  const groupOptions = [
    unassignedOption,
    ...(group ? [group] : []),
    ...groups.filter((candidate) => candidate.id !== group?.id),
  ];
  const currentGroupName = group?.name ?? unassignedOption.name;

  return (
    <ProviderGroupPicker
      className="min-w-40"
      ariaLabel={`调整 ${resourceLabel} 的分组，当前为 ${currentGroupName}`}
      title={group ? `${group.name} (${group.id})` : "当前未分组，不参与调度"}
      disabled={disabled || groupOptions.length === 1}
      value={group?.id ?? ""}
      groups={groupOptions}
      onChange={onChange}
    />
  );
}
