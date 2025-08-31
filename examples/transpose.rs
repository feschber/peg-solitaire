macro_rules! print_nums {
    ($($e:expr),+ $(,)?) => {
        let nums = [ $($e), + ];
        let labels = [ $(stringify!($e) ), +];
        print_nums(&nums, &labels);
    };
}
fn print_nums(n: &[u64], labels: &[&'static str]) {
    print!(" ");
    for label in labels {
        print!("{label:<28}");
    }
    println!();
    for _ in 0..labels.len() {
        print!("------------------------    ");
    }
    println!();
    for i in 0..8 {
        for n in n {
            for j in 0..8 {
                let idx = i * 8 + j;
                let bit = (n & (1 << idx)) >> idx;
                let bit = if bit == 1 { '1' } else { '.' };
                print!(" {bit} ");
            }

            print!("    ");
        }
        println!()
    }
    println!();
}

fn main() {
    // let n = 0x1 << 8 | 0x3 << 16 | 0x7 << 24 | 0xf << 32 | 0x1f << 40 | 0x3f << 48 | 0x7f << 56;
    // let n = 0x0f0b0b0900000000;
    // let n = 0xffffffffffffffff;
    //
    let n = 0x0f07030100000000;
    print_nums!(n);
    print_nums!(0x00AA00AA00AA00AA, 0x0000CCCC0000CCCC, 0x00000000F0F0F0F0);
    // println!();
    // print_num(0x00AA00AA00AA00AA);
    // println!();
    // print_num(0x0000CCCC0000CCCC);
    // println!();
    // print_num(0x00000000F0F0F0F0);

    let mut x = n;
    let mut t = 0;

    println!("// step 1 transpose single bits");
    print_nums!(x, t);
    print_nums!(x >> 7, x ^ (x >> 7), (x ^ (x >> 7)) & 0x00AA00AA00AA00AA);
    t = (x ^ (x >> 7)) & 0x00AA00AA00AA00AA;
    print_nums!(x, t);
    print_nums!(x, t, (t << 7), x ^ t, x ^ t ^ (t << 7));
    x = x ^ t ^ (t << 7);

    println!();
    println!();
    println!();

    println!("// step 2 transpose 2x2 fields");
    print_nums!(x, t);
    print_nums!(x >> 14, x ^ (x >> 14), (x ^ (x >> 14)) & 0x0000CCCC0000CCCC);
    t = (x ^ (x >> 14)) & 0x0000CCCC0000CCCC;

    print_nums!(x, t);
    print_nums!(x ^ t, (t << 14), x ^ (t << 14));
    x = x ^ t ^ (t << 14);

    println!();
    println!();
    println!();

    println!("// step 3 transpose 4x4 field");
    print_nums!(x, t);
    print_nums!(x >> 28, x ^ (x >> 28), (x ^ (x >> 28)) & 0x00000000F0F0F0F0);
    t = (x ^ (x >> 28)) & 0x00000000F0F0F0F0;

    print_nums!(x, t);
    print_nums!(x ^ t, (t << 28), x ^ (t << 28));
    x = x ^ t ^ (t << 28);

    print_nums!(x, t);
}
