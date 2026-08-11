from . import db as db
from . import incr as incr
from . import model as model
from . import parse as parse
from . import room as room
from . import sync as sync

def set_config(path: str) -> None:
    """指定 DbOption 配置文件路径（须先于一切触碰配置的调用）。"""

def connect(config: str | None = None, cwd: str | None = None) -> None:
    """连接层初始化：连 SUL_DB，不拿单实例锁；可与在跑服务共存。"""

def full_init(config: str | None = None, cwd: str | None = None) -> None:
    """执行层初始化：拿项目单实例锁 + run_cli 前置段；须先停服务。"""
