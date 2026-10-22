SKIPMOUNT=false
PROPFILE=true
POSTFSDATA=true
LATESTARTSERVICE=true

set_perm_recursive $MODPATH 0 0 0777 0777

ui_print "系统架构为：$ARCH"
ui_print "系统 SDK 版本：$API"
ui_print "EasyTier 安装位置：/data/adb/modules/easytier_magisk"
ui_print "配置文件位置：/data/adb/modules/easytier_magisk/config/config.toml"
ui_print "----------------------------------"
ui_print "----------------------------------"
ui_print "修改配置后无需重启设备，在 Magisk 中禁用 EasyTier 模块，等待 10 秒后重新启用即可让新配置生效"
ui_print "点击 Magisk 中模块左下角的“操作”按钮可以禁用或激活热点子网转发，使用该功能前需要在配置中提前配置好 cidr 参数"
ui_print "模块安装完成，重启设备生效"
