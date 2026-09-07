import json
import equi8000_live as e
print(json.dumps(e.sql('SELECT dbnum, owner_project, file_path, db_type, applied_sesno FROM dbnum_watermark:8000;'),ensure_ascii=False))
print(json.dumps(e.sql('SELECT * FROM model_update_pending;'),ensure_ascii=False))
