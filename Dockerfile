# 使用 debian 瘦身版作为运行环境
FROM debian:bookworm-slim

# 安装运行所需的底层依赖（如 openssl, ca-certificates）
RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 \
    && rm -rf /var/lib/apt/lists/*

# 设置容器内的工作目录
WORKDIR /app

# 声明表格中的环境变量并设置默认值
ENV SERVER_HOST=0.0.0.0 \
    SERVER_PORT=30010 \
    DB_PATH=./data/earthquake.db \
    BARK_API_URL=https://api.day.app \
    MAX_CONCURRENT_NOTIFICATIONS=1000 \
    BATCH_SIZE=5000 \
    HTTP_POOL_SIZE=200

# 声明开放 30010 端口
EXPOSE 30010

# 将 GitHub Actions 下载下来的 Linux 二进制文件复制到容器的工作目录下
COPY earthquake-alert-backend-linux /app/earthquake-alert-backend

# 确保二进制文件有执行权限
RUN chmod +x /app/earthquake-alert-backend

# 启动程序（由于设置了 WORKDIR /app，程序会在 /app 下执行，并能正确读取同路径下的 .env 文件）
CMD ["./earthquake-alert-backend"]