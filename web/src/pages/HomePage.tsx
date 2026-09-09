import {
  ArrowDown,
  ArrowRight,
  ArrowUpRight,
  AudioLines,
  Blocks,
  Check,
  Code2,
  Fingerprint,
  Image,
  Layers3,
  Search,
  ShieldCheck,
  Terminal,
  Users,
  Workflow,
} from "lucide-react";
import openaiLogo from "@lobehub/icons-static-svg/icons/openai.svg";
import claudeLogo from "@lobehub/icons-static-svg/icons/claude.svg";
import tokenGatewayLogo from "../assets/token-gateway-logo.svg";
import { consolePagePaths } from "../config";
import { ProductPreview } from "../features/home/ProductPreview";
import "./home.css";

const consoleEntryPath = consolePagePaths.usage;

function Brand() {
  return (
    <a className="home-brand" href="/" aria-label="Aestus 首页">
      <img src={tokenGatewayLogo} alt="" />
      <span>
        aestus<span className="brand-period">.</span>
      </span>
    </a>
  );
}

function GatewayDiagram() {
  return (
    <div
      className="gateway-diagram"
      role="img"
      aria-label="团队成员、应用和 Agent 通过各自的网关 Key，经 Aestus 工作空间授权，使用 GPT 与 Claude 账号池。示意图。"
    >
      <div className="diagram-heading">
        <span>THE WORKSPACE LAYER</span>
        <span>架构示意 / 01</span>
      </div>
      <div className="diagram-boundary">
        <span>
          <ShieldCheck size={13} />
          独立空间 · 清晰边界
        </span>
      </div>
      <svg
        className="diagram-lines"
        viewBox="0 0 580 380"
        fill="none"
        aria-hidden="true"
      >
        <path d="M114 96H161Q183 96 183 118V168Q183 190 205 190H290M114 190H290M114 284H161Q183 284 183 262V212Q183 190 205 190M290 190H370Q392 190 392 168V139Q392 117 414 117H472M370 190Q392 190 392 212V241Q392 263 414 263H472" />
        <path
          className="flow-path"
          d="M114 190H370Q392 190 392 168V139Q392 117 414 117H472"
        />
        <circle cx="183" cy="190" r="4" />
        <circle cx="392" cy="190" r="4" />
      </svg>
      <div className="diagram-input input-one">
        <Users size={17} />
        <span>团队成员</span>
      </div>
      <div className="diagram-input input-two">
        <Terminal size={17} />
        <span>业务应用</span>
      </div>
      <div className="diagram-input input-three">
        <Workflow size={17} />
        <span>团队 Agent</span>
      </div>
      <div className="gateway-core">
        <img src={tokenGatewayLogo} alt="" />
        <span>AESTUS</span>
      </div>
      <div className="gateway-caption">一个入口，有序协作</div>
      <div className="diagram-provider provider-gpt">
        <img src={openaiLogo} alt="" />
        <div>
          <strong>GPT</strong>
          <span>托管账号池</span>
        </div>
      </div>
      <div className="diagram-provider provider-claude">
        <img src={claudeLogo} alt="" />
        <div>
          <strong>Claude</strong>
          <span>托管账号池</span>
        </div>
      </div>
      <div className="diagram-event">
        <span>
          <Check size={13} />
          账号托管
        </span>
        <span>
          <Check size={13} />
          分组授权
        </span>
        <span>
          <Check size={13} />
          自动调度
        </span>
      </div>
    </div>
  );
}

export function HomePage() {
  return (
    <div className="aestus-home">
      <a className="home-skip-link" href="#main-content">
        跳到主要内容
      </a>
      <header className="home-header">
        <div className="home-container home-header-inner">
          <Brand />
          <nav className="home-header-nav" aria-label="主导航">
            <span>文档</span>
            <span>定价</span>
            <span>咨询</span>
          </nav>
          <a className="home-console-link" href={consoleEntryPath}>
            进入控制台 <ArrowUpRight size={16} />
          </a>
        </div>
      </header>
      <main id="main-content">
        <section
          className="home-hero home-container"
          aria-labelledby="hero-title"
        >
          <div className="hero-copy">
            <div className="home-eyebrow">
              <span className="eyebrow-line" />
              YOUR ACCOUNTS. YOUR WORKSPACE.
            </div>
            <h1 id="hero-title">
              你的 AI 资源，
              <br />
              团队的<span>创造力。</span>
            </h1>
            <p className="hero-description">
              在专属工作空间，统一托管 GPT 与 Claude 账号。
              <br className="desktop-break" />
              让资源有序流转，让团队专注创造。
            </p>
            <div className="hero-actions">
              <a
                className="home-button home-button-dark"
                href={consoleEntryPath}
              >
                开始账号托管 <ArrowUpRight size={18} />
              </a>
              <a className="home-text-link" href="#product">
                看看如何运作 <ArrowDown size={16} />
              </a>
            </div>
            <div className="hero-notes">
              <span>
                <Check size={14} />
                自有账号托管
              </span>
              <span>
                <Check size={14} />
                空间资源隔离
              </span>
              <span>
                <Check size={14} />
                成员独立 Key
              </span>
            </div>
          </div>
          <GatewayDiagram />
          <div className="hero-bottom">
            <span>为团队连接 AI 的每一种可能</span>
            <div>
              <span>
                <img src={openaiLogo} alt="" />
                OpenAI
              </span>
              <span>
                <img src={claudeLogo} alt="" />
                Anthropic
              </span>
              <span className="hero-protocol-note">标准协议接入</span>
            </div>
            <a href="#product" aria-label="向下查看产品预览">
              <ArrowDown size={17} />
            </a>
          </div>
        </section>

        <ProductPreview />

        <section
          id="capabilities"
          className="home-capabilities home-container"
          aria-labelledby="capabilities-title"
        >
          <div className="home-section-heading">
            <div>
              <div className="home-eyebrow">
                <span>02</span> 为团队而设计
              </div>
              <h2 id="capabilities-title">
                共享 AI 能力。
                <br />
                <span>保留清晰边界。</span>
              </h2>
            </div>
            <p>
              从一个人的账号，到整个团队的生产力。
              <br />
              每一份资源，都有合适的使用方式。
            </p>
          </div>
          <div className="capability-grid">
            <article className="capability-card capability-main">
              <div className="feature-icon">
                <Fingerprint size={24} strokeWidth={1.5} />
              </div>
              <div className="feature-kicker">权限清晰，协作从容</div>
              <h3>
                共享能力，
                <br />
                无需共享凭证。
              </h3>
              <p>
                成员通过独立的网关 Key 调用
                AI。管理员按资源组授权，设置额度与并发，让每个人获得所需的能力。
              </p>
              <div
                className="access-visual"
                role="img"
                aria-label="研发与创意成员分别通过独立 Key，访问各自获得授权的资源组"
              >
                <div className="access-visual-top">
                  <ShieldCheck size={16} />
                  <span>Studio 工作空间</span>
                  <span>按组授权</span>
                </div>
                <div className="access-row">
                  <span className="access-avatar">D</span>
                  <span>
                    研发成员<small>独立网关 Key</small>
                  </span>
                  <ArrowRight size={17} />
                  <span className="access-group">研发资源组</span>
                </div>
                <div className="access-row">
                  <span className="access-avatar">C</span>
                  <span>
                    创意成员<small>独立网关 Key</small>
                  </span>
                  <ArrowRight size={17} />
                  <span className="access-group">创意资源组</span>
                </div>
              </div>
            </article>
            <article className="capability-card">
              <div className="feature-icon">
                <Layers3 size={23} strokeWidth={1.5} />
              </div>
              <div className="feature-kicker">资源集中，维护自动</div>
              <h3>把日常维护，交给系统。</h3>
              <p>
                集中管理 OAuth 账号与官方 API
                Key。账号凭证自动刷新，可用资源自动调度，减少团队的管理负担。
              </p>
              <div className="maintenance-flow">
                <span>
                  <Check size={13} />
                  凭证刷新
                </span>
                <ArrowRight size={15} />
                <span>资源调度</span>
                <ArrowRight size={15} />
                <span>会话保持</span>
              </div>
            </article>
            <article className="capability-card">
              <div className="feature-icon">
                <AudioLines size={23} strokeWidth={1.5} />
              </div>
              <div className="feature-kicker">调用可查，用量可见</div>
              <h3>让资源消耗，有据可循。</h3>
              <p>
                按用户、模型和 API Key
                理解用量。管理员掌握空间内的调用，成员专注自己的使用记录。
              </p>
              <a href="#product" className="home-text-link">
                探索工作空间 <ArrowUpRight size={16} />
              </a>
            </article>
          </div>
        </section>

        <section
          id="integration"
          className="integration-section"
          aria-labelledby="integration-title"
        >
          <div className="home-container">
            <div className="home-section-heading">
              <div>
                <div className="home-eyebrow">
                  <span>03</span> 标准接入，自由扩展
                </div>
                <h2 id="integration-title">
                  沿用熟悉的方式。
                  <br />
                  <span>构建自己的可能。</span>
                </h2>
              </div>
              <p>
                以标准协议连接应用。
                <br />以 WASM 插件，定义处理逻辑。
              </p>
            </div>
            <div className="integration-grid">
              <article className="integration-card">
                <div className="integration-card-label">
                  <Code2 size={20} />
                  <span>STANDARD PROTOCOLS</span>
                </div>
                <h3>
                  熟悉的 API，
                  <br />
                  更丰富的 AI 能力。
                </h3>
                <p>
                  配置网关地址与 Key，将 OpenAI 与 Anthropic
                  风格接口接入你的应用和工作流。
                </p>
                <div className="protocol-interface">
                  <div className="protocol-endpoint">
                    <img src={openaiLogo} alt="" />
                    <span>OpenAI</span>
                    <code>/v1/responses</code>
                  </div>
                  <div className="protocol-endpoint">
                    <img src={claudeLogo} alt="" />
                    <span>Anthropic</span>
                    <code>/v1/messages</code>
                  </div>
                  <div className="protocol-capabilities">
                    <span>
                      <Terminal size={14} />
                      文本生成
                    </span>
                    <span>
                      <Image size={14} />
                      图片生成与编辑
                    </span>
                    <span>
                      <Search size={14} />
                      Codex 搜索
                    </span>
                  </div>
                </div>
                <a
                  className="integration-card-link"
                  href={consolePagePaths.gatewayApiKeys}
                >
                  配置 API Key <ArrowUpRight size={17} />
                </a>
              </article>
              <article className="integration-card wasm-feature">
                <div className="integration-card-label">
                  <Blocks size={20} />
                  <span>WEBASSEMBLY PLUGINS</span>
                </div>
                <h3>
                  你的业务，
                  <br />
                  你的处理逻辑。
                </h3>
                <p>
                  将请求与响应转换组合成插件套件，按 Key
                  绑定，为协议适配与业务定制留出空间。
                </p>
                <div
                  className="wasm-visual"
                  role="img"
                  aria-label="WASM 套件提供请求转换插槽，并按交付模式选择非流式或流式响应转换插槽"
                >
                  <div className="wasm-module">
                    <Blocks size={27} strokeWidth={1.5} />
                    <div>
                      <strong>WASM</strong>
                      <span>自定义插件套件</span>
                    </div>
                    <code>.wasm</code>
                  </div>
                  <div className="wasm-slots">
                    <div>
                      <Code2 size={17} />
                      <span>请求转换</span>
                    </div>
                    <div>
                      <Layers3 size={17} />
                      <span>非流式响应</span>
                    </div>
                    <div>
                      <AudioLines size={17} />
                      <span>流式响应</span>
                    </div>
                  </div>
                </div>
                <div className="wasm-footer">
                  <a
                    className="integration-card-link"
                    href={consolePagePaths.plugins}
                  >
                    管理插件 <ArrowUpRight size={17} />
                  </a>
                  <span>GPT / Claude · OAuth 文本调用</span>
                </div>
              </article>
            </div>
          </div>
        </section>

        <section
          className="final-cta home-container"
          aria-labelledby="cta-title"
        >
          <div>
            <div className="home-eyebrow">
              <span className="eyebrow-line" />
              BETTER TOGETHER.
            </div>
            <h2 id="cta-title">
              下一次创造，
              <br />
              <span>从有序协作开始。</span>
            </h2>
          </div>
          <div className="final-cta-action">
            <a className="home-button home-button-dark" href={consoleEntryPath}>
              进入你的工作空间 <ArrowUpRight size={18} />
            </a>
            <p>自有账号 · 独立空间 · 团队共享</p>
          </div>
        </section>
      </main>
      <footer className="home-footer home-container">
        <Brand />
        <p>让 AI 资源，成为团队的创造力。</p>
        <span>© {new Date().getFullYear()} Aestus</span>
        <a href="#main-content">
          回到顶部 <ArrowUpRight size={14} />
        </a>
      </footer>
    </div>
  );
}
