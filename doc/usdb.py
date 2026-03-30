btc_price = 100000
total_release = 0
month_count = 1
month_release = 0
btc_total = 100

def cacl_usdb(already_rease,btc_total):
 
    usdb_total = btc_total * btc_price
    usdb_release = (usdb_total - already_rease) / (36*30*24*60*5)
    return usdb_release

for i in range(1, 36*30*24*60*5):
    this_release = cacl_usdb(total_release,btc_total)
    btc_total = btc_total + 100/(30*24*60*5)
    if i % (30*24*60*5) == 0:
        print(f"第{month_count}个月，释放{month_release}个USDB,btc价格:{btc_price}")
        month_count += 1
        month_release = 0
        btc_price = btc_price * 1.02
    total_release += this_release
    month_release += this_release
    