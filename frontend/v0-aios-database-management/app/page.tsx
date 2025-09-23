export default function HomePage() {
  return (
    <div className="mx-auto w-full max-w-6xl space-y-6 px-6 py-10">
      <header className="space-y-1">
        <h1 className="text-3xl font-bold text-foreground">AIOS 数据库管理平台</h1>
        <p className="text-sm text-muted-foreground">
          欢迎使用。通过侧边导航可以快速进入部署站点、模型生成及解析向导等功能。
        </p>
      </header>

      <section className="grid gap-4 sm:grid-cols-2 lg:grid-cols-3">
        <article className="rounded-xl border border-border bg-card p-5 shadow-sm">
          <h2 className="text-lg font-semibold text-foreground">部署站点</h2>
          <p className="mt-2 text-sm text-muted-foreground">
            查看站点状态、执行健康检查并创建后台任务。
          </p>
          <a
            href="/sites"
            className="mt-4 inline-flex items-center text-sm font-medium text-primary hover:underline"
          >
            前往部署站点 →
          </a>
        </article>

        <article className="rounded-xl border border-border bg-card p-5 shadow-sm">
          <h2 className="text-lg font-semibold text-foreground">生成模型</h2>
          <p className="mt-2 text-sm text-muted-foreground">
            选择 dbnum 或输入 refno，一键生成 OBJ 模型并下载。
          </p>
          <a
            href="/model"
            className="mt-4 inline-flex items-center text-sm font-medium text-primary hover:underline"
          >
            开始生成 →
          </a>
        </article>

        <article className="rounded-xl border border-border bg-card p-5 shadow-sm">
          <h2 className="text-lg font-semibold text-foreground">解析向导</h2>
          <p className="mt-2 text-sm text-muted-foreground">
            根据项目目录快速创建解析任务，支持并行策略与输出配置。
          </p>
          <a
            href="/wizard"
            className="mt-4 inline-flex items-center text-sm font-medium text-primary hover:underline"
          >
            打开向导 →
          </a>
        </article>
      </section>
    </div>
  )
}
