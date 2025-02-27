// 1/10 = x/2^128
// 1 * 2^128 / 10 = x

// pub const DECIMAL_LENGTH: usize = 48;

// #[derive(Debug, Clone)]
// pub struct MyDecimal {
//     digits: [u8; DECIMAL_LENGTH],
//     scale: u8,
// }

// impl MyDecimal {
//     pub fn zero() -> Self {
//         Self {
//             digits: [0u8; DECIMAL_LENGTH],
//             scale: 0,
//         }
//     }

//     pub fn to_string_radix(&self, radix: u8) -> String {
//         // let mut s = String::with_capacity(
//         //     DECIMAL_LENGTH * 8 / radix.next_power_of_two().checked_ilog2().unwrap() as usize,
//         // );

//         // let mut rem: u8;

//         // for d in digits {}
//         todo!()
//     }

//     fn get_place(&self, place: i16) -> u8 {
//         let index: i16 = self.scale as i16 + place;
//         if index < 0 || index > DECIMAL_LENGTH as i16 {
//             0
//         } else {
//             self.digits[index as usize]
//         }
//     }

//     fn top_index(&self) -> u8 {
//         for i in DECIMAL_LENGTH..=0 {
//             if self.digits[i] != 0 {
//                 return i as u8;
//             }
//         }

//         0
//     }

//     fn set_place(&mut self, place: i16, x: u8) -> bool {
//         let mut index: i16 = self.scale as i16 + place;

//         if index < 0 {
//             let can_shift = DECIMAL_LENGTH as i16 - 1 - self.top_index() as i16;
//             if index + can_shift < 0 {
//                 return false;
//             }

//             self.digits.rotate_right((-index) as usize);
//             index = 0;
//         }

//         self.digits[index as usize] = x;
//         true
//     }
// }

// #[cfg(target_family = "wasm")]
// #[inline]
// fn die(msg: &str) -> ! {
//     use near_sdk::env;

//     env::panic_str(msg)
// }

// #[cfg(not(target_family = "wasm"))]
// #[inline]
// fn die(msg: &str) -> ! {
//     panic!("{msg}")
// }

// impl PartialOrd for MyDecimal {
//     fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
//         todo!()
//     }
// }

// impl PartialEq for MyDecimal {
//     fn eq(&self, other: &Self) -> bool {
//         let low = -i16::from(self.scale.max(other.scale));
//         let high = DECIMAL_LENGTH as i16 - i16::from(self.scale.min(other.scale));

//         for p in low..high {
//             if self.get_place(p) != other.get_place(p) {
//                 return false;
//             }
//         }

//         true
//     }
// }

// impl Eq for MyDecimal {}

// impl Add for MyDecimal {
//     type Output = Self;

//     fn add(self, rhs: Self) -> Self::Output {
//         let mut out = Self::zero();
//         out.scale = self.scale.max(rhs.scale);

//         let mut carry = false;

//         for i in 0..DECIMAL_LENGTH {
//             let place = i as i16 - out.scale as i16;
//             let mut digit = carry as u8;
//             (digit, carry) = digit.overflowing_add(self.get_place(place));

//             let c;
//             (digit, c) = digit.overflowing_add(rhs.get_place(place));
//             carry |= c;

//             out.digits[i] = digit;
//         }

//         if carry {
//             die("overflow")
//         }

//         out
//     }
// }

// macro_rules! impl_from_uint {
//     ($t:ty) => {
//         impl From<$t> for MyDecimal {
//             fn from(value: $t) -> Self {
//                 let mut digits = [0u8; DECIMAL_LENGTH];
//                 digits.copy_from_slice(&value.to_le_bytes());
//                 Self { digits, scale: 0 }
//             }
//         }
//     };
// }

// impl_from_uint!(u8);
// impl_from_uint!(u16);
// impl_from_uint!(u32);
// impl_from_uint!(u64);
// impl_from_uint!(u128);

// macro_rules! impl_try_from_for_uint {
//     ($t:ty) => {
//         impl TryFrom<MyDecimal> for $t {
//             type Error = ();

//             fn try_from(value: MyDecimal) -> Result<Self, Self::Error> {
//                 let should_be_empty = &value.digits[value.scale + 1..];

//                 for t in should_be_empty {
//                     if *t != 0 {
//                         return Err(());
//                     }
//                 }

//                 let mut bytes = [0u8; size_of::<$t>()];
//                 bytes.copy_from_slice(&value.digits[value.scale..]);
//                 Ok(<$t>::from_le_bytes(bytes))
//             }
//         }
//     };
// }

// impl_try_from_for_uint!(u8);
// impl_try_from_for_uint!(u16);
// impl_try_from_for_uint!(u32);
// impl_try_from_for_uint!(u64);
// impl_try_from_for_uint!(u128);

// #[derive(Clone, Debug)]
// #[near(serializers = [json, borsh])]
// pub struct WrappedBigDecimal(
//     #[borsh(serialize_with = "serialize", deserialize_with = "deserialize")] pub BigDecimal,
// );

// impl Deref for WrappedBigDecimal {
//     type Target = BigDecimal;

//     fn deref(&self) -> &Self::Target {
//         &self.0
//     }
// }

// impl From<BigDecimal> for WrappedBigDecimal {
//     fn from(value: BigDecimal) -> Self {
//         Self(value)
//     }
// }

// impl From<WrappedBigDecimal> for BigDecimal {
//     fn from(value: WrappedBigDecimal) -> Self {
//         value.0
//     }
// }

// fn serialize<W: borsh::io::Write>(b: &BigDecimal, writer: &mut W) -> Result<(), borsh::io::Error> {
//     let (bigint, scale) = b.as_bigint_and_scale();
//     let bigint_bytes = bigint.to_signed_bytes_le();
//     borsh::BorshSerialize::serialize(&bigint_bytes, writer)?;
//     borsh::BorshSerialize::serialize(&scale, writer)?;
//     Ok(())
// }

// fn deserialize<R: borsh::io::Read>(reader: &mut R) -> Result<BigDecimal, borsh::io::Error> {
//     let bigint_bytes: Vec<u8> = borsh::BorshDeserialize::deserialize_reader(reader)?;
//     let scale: i64 = borsh::BorshDeserialize::deserialize_reader(reader)?;
//     let bigint = BigInt::from_signed_bytes_le(&bigint_bytes);
//     Ok(BigDecimal::from_bigint(bigint, scale))
// }

// #[cfg(test)]
// mod tests {
//     use std::str::FromStr;

//     use bigdecimal::BigDecimal;
//     use near_sdk::borsh;

//     use super::WrappedBigDecimal;

//     #[test]
//     pub fn test() {
//         let my_bigdecimal = BigDecimal::from_str("1.23456789").unwrap();
//         let borshed = borsh::to_vec(&WrappedBigDecimal(my_bigdecimal.clone())).unwrap();
//         let parsed: WrappedBigDecimal = borsh::from_slice(&borshed).unwrap();
//         assert_eq!(my_bigdecimal, parsed.into());
//     }
// }
