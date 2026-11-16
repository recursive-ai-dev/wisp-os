# collections.wisp -- list and map copying, zipping, grouping, and deduplication

fn copy_list(xs) {
    let out = [];
    for x in xs {
        push(out, x);
    }
    return out;
}

fn copy_map(m) {
    let out = {};
    for k in keys(m) {
        out[k] = m[k];
    }
    return out;
}

fn zip(a, b) {
    let n = min(len(a), len(b));
    let out = [];
    for i in range(n) {
        push(out, [a[i], b[i]]);
    }
    return out;
}

fn enum_list(xs) {
    return enumerate(xs);
}

fn group_by(xs, key_fn) {
    let groups = {};
    for x in xs {
        let k = key_fn(x);
        let bucket = get(groups, k, nil);
        if bucket == nil {
            groups[k] = [x];
        } else {
            push(bucket, x);
        }
    }
    return groups;
}

fn unique(xs) {
    let out = [];
    for x in xs {
        if not contains(out, x) {
            push(out, x);
        }
    }
    return out;
}

fn flatten(xs) {
    let out = [];
    for sub in xs {
        if type(sub) == "list" {
            for item in sub {
                push(out, item);
            }
        } else {
            push(out, sub);
        }
    }
    return out;
}

return {
    "copy_list": copy_list,
    "copy_map": copy_map,
    "zip": zip,
    "enumerate": enum_list,
    "group_by": group_by,
    "unique": unique,
    "flatten": flatten
};
