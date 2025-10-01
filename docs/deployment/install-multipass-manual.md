# Multipass 手动安装指南

## 问题

brew 下载速度很慢（网络问题），已下载 30MB 但未完成（安装包约 100MB+）。

## 解决方案 1: 手动下载并安装（最快）

### 步骤 1: 下载安装包

**官方下载地址**:
```
https://github.com/canonical/multipass/releases/download/v1.16.1/multipass-1.16.1+mac-Darwin.pkg
```

### 步骤 2: 安装

下载完成后，双击 `.pkg` 文件安装，或使用命令行：

```bash
sudo installer -pkg ~/Downloads/multipass-1.16.1+mac-Darwin.pkg -target /
```

### 步骤 3: 验证安装

```bash
multipass version
```

预期输出：
```
multipass   1.16.1+mac
multipassd  1.16.1+mac
```

---

## 解决方案 2: 继续等待 brew（慢）

```bash
# 在新终端运行，等待完成
HOMEBREW_NO_AUTO_UPDATE=1 brew install --cask multipass

# 监控下载进度
ls -lh ~/Library/Caches/Homebrew/downloads/*multipass*.pkg*
```

---

## 解决方案 3: 使用国内镜像（如果可用）

```bash
# 如果配置了 brew 镜像源，可能会快一些
brew install --cask multipass
```

---

## 安装完成后的下一步

安装成功后，请告知我，我将继续：

1. ✅ 创建主节点虚拟机
2. ✅ 创建副本节点虚拟机
3. ✅ 传输文件到虚拟机
4. ✅ 安装和配置 LiteFS
5. ✅ 测试分布式同步

---

## 快速命令参考

```bash
# 创建虚拟机（安装完成后运行）
multipass launch --name litefs-primary --memory 4G --disk 20G
multipass launch --name litefs-replica1 --memory 2G --disk 10G

# 查看虚拟机列表
multipass list

# 进入虚拟机
multipass shell litefs-primary
```