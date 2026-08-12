from . import db as db
from . import fixture as fixture
from . import incr as incr
from . import model as model
from . import parse as parse
from . import room as room
from . import spatial as spatial
from . import sync as sync

def set_config(path: str) -> None:
    """指定 DbOption 配置文件路径（须先于一切触碰配置的调用）。"""

def connect(config: str | None = None, cwd: str | None = None) -> None:
    """连接层初始化：连 SUL_DB，不拿单实例锁；可与在跑服务共存。"""

def full_init(
    config: str | None = None, cwd: str | None = None, force: bool = False
) -> None:
    """执行层初始化：拿项目单实例锁 + run_cli 前置段；须先停服务。

    拿锁后还会探本机 http_api_addr / 8022 / 9099 的 /api/v1/health，发现同工程
    的活服务就拒绝（锁按项目根隔离，挡不住跨部署互踩）。force=True 跳过探测。
    """
