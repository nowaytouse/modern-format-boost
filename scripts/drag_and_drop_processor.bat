@echo off
REM Modern Format Boost - Windows Drag & Drop Processor
REM 拖拽式一键处理脚本 (Windows版)
REM 
REM 使用方法：将文件夹拖拽到此脚本上，或双击后输入文件夹路径
REM Usage: Drag folder to this script, or double-click and input folder path

setlocal enabledelayedexpansion
chcp 65001 >nul

REM 获取脚本所在目录
set "SCRIPT_DIR=%~dp0"
set "PROJECT_ROOT=%SCRIPT_DIR%.."

REM 工具路径
set "IMGQUALITY_HEVC=%PROJECT_ROOT%\imgquality_hevc\target\release\imgquality-hevc.exe"
set "VIDQUALITY_HEVC=%PROJECT_ROOT%\vidquality_hevc\target\release\vidquality-hevc.exe"

REM 显示欢迎信息
echo 🚀 Modern Format Boost - 一键处理器 (Windows)
echo ==================================================
echo 📁 处理模式：原地转换（删除原文件）
echo 🔧 图像参数：--in-place --recursive --match-quality --explore
echo 🎬 视频参数：--in-place --recursive --match-quality true --explore
echo ==================================================
echo.

REM 检查工具是否存在
if not exist "%IMGQUALITY_HEVC%" (
    echo ❌ imgquality-hevc.exe not found. Building...
    cd /d "%PROJECT_ROOT%"
    cargo build --release -p imgquality-hevc
    if errorlevel 1 (
        echo ❌ 编译失败，请确保已安装 Rust 和相关依赖
        pause
        exit /b 1
    )
)

if not exist "%VIDQUALITY_HEVC%" (
    echo ❌ vidquality-hevc.exe not found. Building...
    cd /d "%PROJECT_ROOT%"
    cargo build --release -p vidquality-hevc
    if errorlevel 1 (
        echo ❌ 编译失败，请确保已安装 Rust 和相关依赖
        pause
        exit /b 1
    )
)

REM 获取目标目录
if "%~1"=="" (
    REM 交互模式
    echo 请将要处理的文件夹拖拽到此窗口，然后按回车：
    echo 或者直接输入文件夹路径：
    set /p "TARGET_DIR="
) else (
    REM 拖拽模式
    set "TARGET_DIR=%~1"
)

REM 去除引号
set "TARGET_DIR=%TARGET_DIR:"=%"

REM 验证目录
if not exist "%TARGET_DIR%" (
    echo ❌ 错误：目录不存在: %TARGET_DIR%
    pause
    exit /b 1
)

echo 📂 目标目录: %TARGET_DIR%

REM 安全检查
echo.
echo ⚠️  即将开始原地处理（会删除原文件）：
echo    目录: %TARGET_DIR%
echo    模式: 递归处理所有子目录
echo    参数: --match-quality --explore
echo.
set /p "CONFIRM=确认继续？(y/N): "

if /i not "%CONFIRM%"=="y" (
    echo ❌ 用户取消操作
    pause
    exit /b 0
)

REM 统计文件数量
echo 📊 正在统计文件...

REM 计算图像文件数量
set IMG_COUNT=0
for /r "%TARGET_DIR%" %%f in (*.jpg *.jpeg *.png *.gif *.bmp *.tiff *.webp *.heic) do (
    set /a IMG_COUNT+=1
)

REM 计算视频文件数量
set VID_COUNT=0
for /r "%TARGET_DIR%" %%f in (*.mp4 *.mov *.avi *.mkv *.webm *.m4v *.flv) do (
    set /a VID_COUNT+=1
)

set /a TOTAL_COUNT=IMG_COUNT+VID_COUNT

echo    🖼️  图像文件: %IMG_COUNT%
echo    🎬 视频文件: %VID_COUNT%
echo    📁 总计: %TOTAL_COUNT%

if %TOTAL_COUNT%==0 (
    echo ❌ 未找到支持的媒体文件
    pause
    exit /b 1
)

REM 处理图像文件
if %IMG_COUNT% gtr 0 (
    echo.
    echo 🖼️  开始处理图像文件...
    echo ==================================================
    
    "%IMGQUALITY_HEVC%" auto "%TARGET_DIR%" --in-place --recursive --match-quality --explore
    
    if errorlevel 1 (
        echo ❌ 图像处理失败
        pause
        exit /b 1
    )
    
    echo ✅ 图像处理完成
)

REM 处理视频文件
if %VID_COUNT% gtr 0 (
    echo.
    echo 🎬 开始处理视频文件...
    echo ==================================================
    
    "%VIDQUALITY_HEVC%" auto "%TARGET_DIR%" --in-place --recursive --match-quality true --explore
    
    if errorlevel 1 (
        echo ❌ 视频处理失败
        pause
        exit /b 1
    )
    
    echo ✅ 视频处理完成
)

REM 显示完成信息
echo.
echo 🎉 处理完成！
echo ==================================================
echo 📁 处理目录: %TARGET_DIR%
echo 🖼️  图像文件: %IMG_COUNT%
echo 🎬 视频文件: %VID_COUNT%
echo ==================================================
echo.
pause