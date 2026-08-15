/**
 * echarts-gl 2.1.0 尚未随包发布 TypeScript 声明。这里只声明项目按需加载的两个安装器，
 * 运行时实现仍直接来自官方包，不复制或维护第三方图表逻辑。
 */
declare module "echarts-gl/charts" {
  export const Bar3DChart: (registers: any) => void;
}

declare module "echarts-gl/components" {
  export const Grid3DComponent: (registers: any) => void;
}
