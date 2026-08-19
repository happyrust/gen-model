# dbnum=8000 CATA 依赖闭包与停滞收口计划

实施以 ADR-034 和 `specs/011-watch-scope-cata-dependency/` 为准。Constitution Check：依赖失败不推进
水位；判定异常不静默；共享扫描先裁决后登记；暂存与水位同一提交单元；每条新增不变量配回归测试，
live 结果进入唯一台账。无新增持久表，限定模式以外的 ADR-025 初始化顺序不变。
