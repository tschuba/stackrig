//! `strig mv|move <stack> --to <host>`: moves the stack's entry between `host_vars/<host>/stacks.yml` files.
use crate::ops;

/// Removes `- name` from the `stacks:` list in `from`, returning the new text.
fn remove(text: &str, name: &str) -> Result<String, String> {
    let mut l: Vec<&str> = text.lines().collect();
    let i = l.iter().position(|x| x.trim() == format!("- {name}")).ok_or("entry not found in stacks.yml (unusual format?)")?;
    l.remove(i);
    // an emptied list must stay valid YAML
    let s = l.iter().position(|x| x.trim_end() == "stacks:").ok_or("no `stacks:` list")?;
    let mut t = l.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    if !l.get(s + 1).is_some_and(|x| x.trim_start().starts_with("- ")) { t[s] = "stacks: []".into(); }
    Ok(t.join("\n") + "\n")
}

/// Appends `- name` after the last item of the `stacks:` list.
pub(crate) fn append(text: &str, name: &str) -> Result<String, String> {
    let mut l: Vec<String> = text.lines().map(String::from).collect();
    if let Some(i) = l.iter().position(|x| x.trim_end() == "stacks: []") { l[i] = "stacks:".into(); }
    let s = l.iter().position(|x| x.trim_end() == "stacks:").ok_or("no `stacks:` list in target stacks.yml")?;
    let mut last = s;
    while l.get(last + 1).is_some_and(|x| x.trim_start().starts_with("- ")) { last += 1; }
    let ind = if last > s { l[last].len() - l[last].trim_start().len() } else { 2 };
    l.insert(last + 1, format!("{}- {name}", " ".repeat(ind)));
    Ok(l.join("\n") + "\n")
}

pub(crate) fn mv(a: &[String], dry: bool) -> Result<(), String> {
    let usage = || "usage: strig mv|move <stack> --to <host> [--dry-run]".to_string();
    let (Some(name), Some("--to"), Some(to), None) = (a.first(), a.get(1).map(String::as_str), a.get(2), a.get(3)) else { return Err(usage()) };
    let r = ops::root();
    let (stacks, _) = ops::load(&r)?;
    let s = stacks.iter().find(|s| &s.name == name).ok_or(format!("unknown stack `{name}`"))?;
    if &s.host == to { return Err(format!("`{name}` is already on {to}")); }
    let f = |h: &str| r.join("host_vars").join(h).join("stacks.yml");
    if !f(to).exists() { return Err(format!("unknown host `{to}` (no {})", f(to).display())); }
    let (old, new) = (std::fs::read_to_string(f(&s.host)).map_err(|e| e.to_string())?, std::fs::read_to_string(f(to)).map_err(|e| e.to_string())?);
    let (old, new) = (remove(&old, name)?, append(&new, name)?);
    if dry { println!("dry run: {name}: {} -> {to}", s.host); return Ok(()); }
    // add first: a failure in between leaves the stack on two hosts (visible in `ls`), never on none
    std::fs::write(f(to), new).map_err(|e| e.to_string())?;
    std::fs::write(f(&s.host), old).map_err(|e| e.to_string())?;
    println!("{name}: {} -> {to} in host_vars. Next: `strig deploy {name}`; then stop it on {} and move its volumes by hand.", s.host, s.host);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn move_entry() {
        assert_eq!(remove("run: 1\nstacks:\n  - a\n  - b\n", "a").unwrap(), "run: 1\nstacks:\n  - b\n");
        assert_eq!(append("stacks:\n  - a\n# c\nx: 1\n", "b").unwrap(), "stacks:\n  - a\n  - b\n# c\nx: 1\n");
        assert_eq!(append("stacks:\n", "b").unwrap(), "stacks:\n  - b\n");
        assert!(remove("stacks:\n  - a\n", "z").is_err());
        assert_eq!(remove("stacks:\n  - a\n", "a").unwrap(), "stacks: []\n");
        assert_eq!(append("run: 1\nstacks: []\n", "b").unwrap(), "run: 1\nstacks:\n  - b\n");
    }
}
