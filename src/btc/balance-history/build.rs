fn main() {
    println!("cargo:rerun-if-env-changed=USDB_BH_REAL_BTC");
    println!("cargo:rustc-check-cfg=cfg(usdb_bh_real_btc)");

    if std::env::var("USDB_BH_REAL_BTC").as_deref() == Ok("1") {
        println!("cargo:rustc-cfg=usdb_bh_real_btc");
    }
}
