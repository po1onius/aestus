import { BarChart, LineChart, PieChart } from "echarts/charts";
import { AriaComponent, GridComponent, LegendComponent, TooltipComponent } from "echarts/components";
import * as echarts from "echarts/core";
import type { ECharts, EChartsCoreOption } from "echarts/core";
import { CanvasRenderer } from "echarts/renderers";
import { useEffect, useRef } from "react";

// 只注册用量页实际使用的图表和组件，避免把完整 ECharts 打入普通用户首屏资源。
echarts.use([
  BarChart,
  LineChart,
  PieChart,
  AriaComponent,
  GridComponent,
  LegendComponent,
  TooltipComponent,
  CanvasRenderer,
]);

export type EChartOption = EChartsCoreOption;

interface EChartProps {
  option: EChartOption;
  ariaLabel: string;
  className?: string;
}

/**
 * ECharts 的 React 生命周期适配层。
 *
 * 图表实例只随 DOM 容器创建和销毁，数据变化使用 setOption 更新；ResizeObserver 负责
 * 侧栏、窗口和响应式网格变化后的尺寸同步，避免各业务图表重复管理实例。
 */
export function EChart({ option, ariaLabel, className = "h-80 min-h-72 w-full" }: EChartProps) {
  const containerRef = useRef<HTMLDivElement | null>(null);
  const chartRef = useRef<ECharts | null>(null);

  useEffect(() => {
    const container = containerRef.current;
    if (!container) {
      return;
    }

    const chart = echarts.init(container, undefined, { renderer: "canvas" });
    const resizeObserver = new ResizeObserver(() => chart.resize());
    chartRef.current = chart;
    resizeObserver.observe(container);

    return () => {
      resizeObserver.disconnect();
      chart.dispose();
      chartRef.current = null;
    };
  }, []);

  useEffect(() => {
    chartRef.current?.setOption(option, { notMerge: true, lazyUpdate: false });
  }, [option]);

  return <div ref={containerRef} className={className} role="img" aria-label={ariaLabel} />;
}
