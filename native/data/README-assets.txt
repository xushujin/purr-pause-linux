=== 胖猫暂停一下（PurrPause） 自定义素材说明 ===

将你的 .webm 视频文件放在此目录下即可替换内置素材。

文件要求：
  - 格式：WebM（VP9 编码，带 Alpha 通道实现透明背景）
  - 背景：必须透明，否则会遮挡桌面
  - 建议尺寸：宽高 200~500px

需要提供两个文件：
  - cat-walk.webm  → 猫走路动画（从右往左走，播放一次）
  - cat-rest.webm  → 猫躺下/休息动画（循环播放）

自定义目录可在「设置 → 素材目录」中指定（需激活）。

制作建议：
  - 使用 FFmpeg 导出带 Alpha 通道的 WebM：
    ffmpeg -i input.mov -c:v libvpx-vp9 -pix_fmt yuva420p -b:v 2M output.webm
  - 走路动画建议 2~4 秒，猫从画面右侧走到左侧
  - 休息动画可以是猫趴着、打呼噜等循环动作

=== 系统要求 ===
本应用需要 libmpv（mpv 播放器库）解码视频。
Debian/Ubuntu: sudo apt install libmpv2
Fedora:        sudo dnf install libmpv
Arch:          sudo pacman -S mpv