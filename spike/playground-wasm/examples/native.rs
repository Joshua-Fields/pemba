//! Native baseline for web/run.mjs: same fixture, same queries, same JSON-string API.
use std::time::Instant;

use pemba_playground_spike::Playground;

fn main() {
    let y = std::fs::read_to_string("../../testdata/fixtures/google_docs.yaml").unwrap();
    let schema = y.split("schema: |-\n").nth(1).unwrap().split("\nrelationships:").next().unwrap().replace("\n  ", "\n");
    let rels: String = y.split("relationships: |-\n").nth(1).unwrap().lines().take_while(|l| l.starts_with("  ") || l.is_empty()).map(|l| l.trim().to_owned() + "\n").collect();
    let pg = Playground::new().ok().unwrap();
    assert!(pg.write_schema(schema.trim_start_matches("  ")).contains("\"ok\":true"));
    println!("{}", pg.write(&rels));
    let mut qs: Vec<String> = y.lines().map(str::trim).filter_map(|l| l.strip_prefix("- ")).filter(|q| q.contains('#') && q.contains('@') && !q.contains(' ')).map(str::to_owned).collect();
    qs.dedup();
    for q in &qs { pg.check(q); }
    let n = 2000;
    let mut per: Vec<f64> = qs.iter().map(|q| { let s = Instant::now(); for _ in 0..n { pg.check(q); } s.elapsed().as_secs_f64() * 1e6 / n as f64 }).collect();
    per.sort_by(|a, b| a.partial_cmp(b).unwrap());
    println!("queries {} check_us min {:.2} p50 {:.2} max {:.2}", qs.len(), per[0], per[per.len() / 2], per[per.len() - 1]);
    let s = Instant::now(); for _ in 0..n { pg.check_with_trace(&qs[0]); } println!("trace_us {:.2}", s.elapsed().as_secs_f64() * 1e6 / n as f64);
    let s = Instant::now(); for _ in 0..n { pg.lookup_resources("document", "view", "user:alice"); } println!("lookup_resources_us {:.2}", s.elapsed().as_secs_f64() * 1e6 / n as f64);
}
