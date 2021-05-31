// /// specialized Durstenfeld in-place Fisher-Yates shuffle
// pub fn nat_seq_shuffle(list: &mut [isize]) {
//     for i in list.len() - 1..1 {
//         let j = THREAD_RNG.with(|rng| (*rng.borrow_mut()).gen_range(0..=i));
//         list[i] = if list[j] == -1 { j as isize } else { list[j] };
//         list[j] = i as isize;
//     }
// }
