use toml_edit::{Array, DocumentMut, value};

pub fn set_bool(document: &mut DocumentMut, key: &str, val: bool) {
    document[key] = value(val);
}

pub fn set_float(document: &mut DocumentMut, key: &str, val: f32) {
    document[key] = value(val as f64);
}

pub fn set_usize(document: &mut DocumentMut, key: &str, val: usize) {
    document[key] = value(val as i64);
}

pub fn set_string_list_value(document: &mut DocumentMut, key: &str, data: &Option<Vec<String>>) {
    if let Some(list) = data {
        let mut array = Array::default();
        for item in list {
            array.push(item.to_owned());
        }
        document[key] = value(array);
    } else {
        document[key] = value(Array::default());
    }
}

pub fn set_u32_list_option(document: &mut DocumentMut, key: &str, data: &Option<Vec<u32>>) {
    if let Some(list) = data {
        let mut array = Array::default();
        for item in list {
            array.push(*item as i64);
        }
        document[key] = value(array);
    } else {
        document.remove(key);
    }
}

pub fn set_string_list_option(document: &mut DocumentMut, key: &str, data: &Option<Vec<String>>) {
    if let Some(list) = data {
        let mut array = Array::default();
        for item in list {
            array.push(item.to_owned());
        }
        document[key] = value(array);
    } else {
        document.remove(key);
    }
}
