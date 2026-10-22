#!/system/bin/sh

MODDIR=${0%/*}
CONFIG_FILE="${MODDIR}/config/config.toml"
LOG_FILE="${MODDIR}/log.log"
MODULE_PROP="${MODDIR}/module.prop"
EASYTIER="${MODDIR}/easytier-core"

REDIR_STATUS=""

# 更新 module.prop 文件中的 description
update_module_description() {
    local status_message=$1
    # 检查 module.prop 文件存在且 description 发生变化了再写入
    if [ -f "${MODULE_PROP}" ]; then
        local current_desc=$(grep "^description=" "${MODULE_PROP}")
        local new_desc="description=[状态] ${status_message}"
        if [ "${current_desc}" != "${new_desc}" ]; then
            sed -i "s#^description=.*#${new_desc}#" "${MODULE_PROP}"
        fi
    fi
}

# 检查并初始化 TUN 设备
if [ ! -e /dev/net/tun ]; then
    if [ ! -d /dev/net ]; then
        mkdir -p /dev/net
    fi
    
    ln -s /dev/tun /dev/net/tun
fi

while true; do
    # 获取子网转发激活状态
    if [ -f "${MODDIR}/enable_IP_rule" ]; then
        REDIR_STATUS="转发已激活"
    else
        REDIR_STATUS="转发已禁用"
    fi

    # 检查模块是否被禁用
    if [ -f "${MODDIR}/disable" ]; then
        update_module_description "主程序已关闭 | ${REDIR_STATUS}"
        if pgrep -f "${EASYTIER}" >/dev/null; then
            echo "开关控制 $(date "+%Y-%m-%d %H:%M:%S") 进程已存在，正在关闭"
            pkill -f "${EASYTIER}"
        fi
        sleep 10s
        continue
    fi
    
    # 检查进程是否已经在运行
    if pgrep -f "${EASYTIER}" >/dev/null; then
        sleep 10s
        continue
    fi
    
    # 检查配置文件是否存在
    if [ ! -f "${CONFIG_FILE}" ]; then
        update_module_description "缺少配置文件"
        sleep 10s
        continue
    fi
    
    "${EASYTIER}" -c "${CONFIG_FILE}" > "${LOG_FILE}" 2>&1 &
    STR_MODE="配置文件模式"

    # 等待进程启动
    sleep 5s
    
    # 启动后的扫尾工作
    if pgrep -f "${EASYTIER}" >/dev/null; then
        
        if ! ip rule show | grep -qE '^[0-9]+:[[:space:]]+from all lookup main$'; then
            ip rule add from all lookup main
        fi
        
        update_module_description "主程序正在运行（${STR_MODE}）| ${REDIR_STATUS}"
    else
        update_module_description "主程序启动失败，请检查配置文件"
    fi
    
    sleep 10s
done
