CREATE design_states SET 
  id = 10,
  code = 'D00',
  name = '前期设计状态',
  level = 'project',
  description = '项⽬管控状态点',
  order_no = 10;

CREATE design_states SET 
  id = 20,
  code = 'D01',
  name = '初始版的⽅案设计固化模型和数据版本',
  level = 'project',
  description = '项⽬管控状态点',
  order_no = 20;

CREATE design_states SET 
  id = 30,
  code = 'D06',
  name = '根据专业需求，中间过程模型和数据版本',
  level = 'department',
  description = '专业所管控状态',
  order_no = 30;

CREATE design_states SET 
  id = 40,
  code = 'D06.20',
  name = '草案/⽅案，仅个⼈可⻅',
  level = 'personal',
  description = '个⼈管控状态点',
  order_no = 40;

CREATE design_states SET 
  id = 50,
  code = 'D06.30',
  name = '草案/⽅案，仅个⼈可⻅',
  level = 'personal',
  description = '个⼈管控状态点',
  order_no = 50;

CREATE design_states SET 
  id = 60,
  code = 'D16',
  name = '⽅案设计阶段⼟建提资点模型和数据版本',
  level = 'project',
  description = '项⽬管控状态点',
  order_no = 60;

CREATE design_states SET 
  id = 70,
  code = 'D19',
  name = '⽅案设计阶段终版模型和数据版本，移交点',
  level = 'project',
  description = '项⽬管控状态点',
  order_no = 70;

CREATE design_states SET 
  id = 80,
  code = 'D20',
  name = '初设阶段初始模型和数据版本，同D19',
  level = 'project',
  description = '项⽬管控状态点',
  order_no = 80;

CREATE design_states SET 
  id = 90,
  code = 'D24',
  name = '初设阶段初步⽅案模型和数据版本',
  level = 'project',
  description = '项⽬管控状态点',
  order_no = 90;

CREATE design_states SET 
  id = 100,
  code = 'D26',
  name = '初步综合完毕的模型和数据版本，作为⼒学提资、计算提资等的记录',
  level = 'department',
  description = '专业所管控状态',
  order_no = 100;

CREATE design_states SET 
  id = 110,
  code = 'D26.20',
  name = '草案/⽅案，仅个⼈可⻅',
  level = 'personal',
  description = '个⼈管控状态点',
  order_no = 110;

CREATE design_states SET 
  id = 120,
  code = 'D30',
  name = '专业模型验证后细化',
  level = 'department',
  description = '专业所管控状态',
  order_no = 120;

CREATE design_states SET 
  id = 130,
  code = 'D32',
  name = '⽀架计算、埋板设计等完成',
  level = 'department',
  description = '专业所管控状态',
  order_no = 130;

CREATE design_states SET 
  id = 140,
  code = 'D36',
  name = '初设阶段D条件提资模型和数据版本',
  level = 'project',
  description = '项⽬管控状态点',
  order_no = 140;

CREATE design_states SET 
  id = 150,
  code = 'D39',
  name = '初设阶段终版模型和数据版本，移交点',
  level = 'project',
  description = '项⽬管控状态点',
  order_no = 150;

CREATE design_states SET 
  id = 160,
  code = 'D40',
  name = '详设阶段初始模型和数据版本',
  level = 'project',
  description = '项⽬管控状态点',
  order_no = 160;

CREATE design_states SET 
  id = 170,
  code = 'D60',
  name = '详设阶段初步模型和数据版本',
  level = 'project',
  description = '项⽬管控状态点',
  order_no = 170;

CREATE design_states SET 
  id = 180,
  code = 'D66',
  name = '专业提资验证完毕，可⽤于综合提资版本',
  level = 'department',
  description = '专业所管控状态',
  order_no = 180;

CREATE design_states SET 
  id = 190,
  code = 'D70',
  name = '详设阶段D条件提资模型和数据版本',
  level = 'project',
  description = '项⽬管控状态点',
  order_no = 190;

CREATE design_states SET 
  id = 200,
  code = 'D76',
  name = '专业提资验证完毕，可⽤于综合提资版本',
  level = 'department',
  description = '专业所管控状态',
  order_no = 200;

CREATE design_states SET 
  id = 210,
  code = 'D80',
  name = '详设阶段建筑、封堵提资模型和数据版本',
  level = 'project',
  description = '项⽬管控状态点',
  order_no = 210;

CREATE design_states SET 
  id = 220,
  code = 'D86',
  name = '专业提资验证完毕，细化基本完成',
  level = 'department',
  description = '专业所管控状态',
  order_no = 220;

CREATE design_states SET 
  id = 230,
  code = 'D90',
  name = '专业模型固化版本，可⾃动出图',
  level = 'project',
  description = '项⽬管控状态点',
  order_no = 230;

CREATE design_states SET 
  id = 240,
  code = 'D99',
  name = '出图完成留存版本,批准后记录ABC版本',
  level = 'project',
  description = '项⽬管控状态点',
  order_no = 240;

CREATE design_states SET 
  id = 250,
  code = 'A/B/C',
  name = '正式版本，移交点',
  level = 'project',
  description = '项⽬管控状态点',
  order_no = 250;

CREATE design_states SET 
  id = 260,
  code = 'SG01/02',
  name = '设计变更版本',
  level = 'project',
  description = '项⽬管控状态点',
  order_no = 260;

CREATE design_states SET 
  id = 270,
  code = 'C00',
  name = '施⼯初始化模型',
  level = 'project',
  description = '项⽬管控状态点',
  order_no = 270;

CREATE design_states SET 
  id = 280,
  code = 'C60',
  name = '施⼯细化模型和数据版本',
  level = 'project',
  description = '项⽬管控状态点',
  order_no = 280;

CREATE design_states SET 
  id = 290,
  code = 'C90',
  name = '施⼯细化固化版本',
  level = 'project',
  description = '项⽬管控状态点',
  order_no = 290;

CREATE design_states SET 
  id = 300,
  code = 'CG01/02',
  name = '现场施⼯变更版本',
  level = 'project',
  description = '项⽬管控状态点',
  order_no = 300;

CREATE design_states SET 
  id = 310,
  code = 'C99',
  name = '施⼯完成版本，竣⼯模型和数据',
  level = 'project',
  description = '项⽬管控状态点',
  order_no = 310;