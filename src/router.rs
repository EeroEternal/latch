use xxhash_rust::xxh3::xxh3_64;

pub fn get_target_node(system_prompt: &str, nodes: &[String]) -> String {
    let hash = xxh3_64(system_prompt.as_bytes());
    let idx = (hash % nodes.len() as u64) as usize;
    nodes[idx].clone()
}
