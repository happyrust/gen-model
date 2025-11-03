@echo off
REM 将 http 地址取自环境变量
if not defined HTTP_ADDR set HTTP_ADDR=http://127.0.0.1:8008
REM database name
if not defined DATABASE_NAME set DATABASE_NAME=AvevaMarineSample
REM namespace
if not defined NAMESPACE set NAMESPACE=1516

REM 定义 surql 文件数组
set surql_files=common.surql

REM 遍历 surql 文件并导入
for %%f in (%surql_files%) do (
    surreal import --conn %HTTP_ADDR% --namespace %NAMESPACE% --database %DATABASE_NAME% -u root -p root %%f
)

REM 调用子文件夹的 bat 文件
REM 切换到子目录并运行相应的 bat 文件

cd gy
call run_gy.bat
cd ..

cd dq
call run_dq.bat
cd ..

cd gy
call run_gy.bat
cd ..

cd yk
call run_yk.bat
cd ..

cd gps
call run_gps.bat
cd ..