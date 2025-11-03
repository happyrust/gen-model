## 版本管理 <br>
### 1.说明 <br>
&emsp; (1) : 目前还在尝试阶段，功能可能会多次调整 <br>
&emsp; (2) : 大小版本的管理，可能需要与增量挂钩，每次增量的数据存放在版本数据表中 <br>
&emsp; (3) : 第一次完全同步的数据为最原始的数据，即第0个大版本，之后增量数据的版本 <br>
&emsp; &emsp; &emsp;都是在第0个大版本的基础上进行的 <br>
&emsp; (4) : 除了最原始的版本，其他所有版本的数据都存放在同一张表内，方便版本数据之间的切换 <br> 

### all_attr_info.json 字段手动修改说明
#### 1. PTCDI -> 更改为 PTCD
#### 2.PAXI -> 类型要修改为 STRING
#### 3.PAAX -> 类型要修改为 STRING
#### 4.PBAX PCAX -> 类型要修改为 STRING
#### 5.LEVE -> 类型要修改为 INTVEC
#### 6.PTS -> 类型要修改为 INTVEC
#### 7.CURD -> 类型要修改为 RefU64Vec 
#### 8.SPCO.DETR -> offset 改为 17
#### 9.ANCI.PTNB -> 类型要修改为 INTEGER 

### 2.版本数据表结构设计与说明

|      字段      |    类型    |                          说明                           |
|:------------:|:--------:|:-----------------------------------------------------:|
|      ID      |  BigINT  |                         主键，自增                         |
|    refno     |  BigINT  |                      pdms的refno                       |
|     大版本      |   INT    |            目前按 1、2、3 ... 自动去设置，后期可能会管理员去设置            |
|     小版本      |   INT    |               与大版本一样，先做成自增，后期需要手动赋值再去修改               |
| pdms_version |   INT    |              pdms数据文件自带的版本，存放在这里以后可能会用到               |
|   operate    | smallint | 该数据在当前小版本做了哪种操作：Modify->0 ;Increment ->1 ;Delete -> 2 |
|     data     |   Blob   |                      该参考号的全部attr                      |

### 3.版本信息表结构设计与说明
|      字段      |     类型      |         说明         |
|:------------:|:-----------:|:------------------:|
| project_name | VARCHAR(20) |        项目名         |
|   version    |     INT     | 当前最新大版本，方便给版本数据表赋值 |
### 4.疑问
&emsp; (1) : 是否需要存放两个data，一个是原始的data，一个是发生修改后的data，这样方便切换版本 <br>


## 编译

### Centos 7的cross build：

cargo zigbuild --release --target x86_64-unknown-linux-gnu.2.17
