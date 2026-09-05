import type { PluginSlot } from "../../types";

export const pluginSlotLabels: Record<PluginSlot, string> = {
  request: "请求插件",
  buffered_response: "非流式响应插件",
  stream_response: "流式响应插件",
};

export const suiteSlotFields = [
  { slot: "request", field: "request_plugin_id" },
  { slot: "buffered_response", field: "buffered_response_plugin_id" },
  { slot: "stream_response", field: "stream_response_plugin_id" },
] as const;
