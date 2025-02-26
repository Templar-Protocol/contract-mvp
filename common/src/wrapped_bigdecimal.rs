use std::ops::Deref;

use bigdecimal::{num_bigint::BigInt, BigDecimal};
use near_sdk::{borsh, near};

#[derive(Clone, Debug)]
#[near(serializers = [json, borsh])]
pub struct WrappedBigDecimal(
    #[borsh(serialize_with = "serialize", deserialize_with = "deserialize")] pub BigDecimal,
);

impl Deref for WrappedBigDecimal {
    type Target = BigDecimal;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl From<BigDecimal> for WrappedBigDecimal {
    fn from(value: BigDecimal) -> Self {
        Self(value)
    }
}

impl From<WrappedBigDecimal> for BigDecimal {
    fn from(value: WrappedBigDecimal) -> Self {
        value.0
    }
}

fn serialize<W: borsh::io::Write>(b: &BigDecimal, writer: &mut W) -> Result<(), borsh::io::Error> {
    let (bigint, scale) = b.as_bigint_and_scale();
    let bigint_bytes = bigint.to_signed_bytes_le();
    borsh::BorshSerialize::serialize(&bigint_bytes, writer)?;
    borsh::BorshSerialize::serialize(&scale, writer)?;
    Ok(())
}

fn deserialize<R: borsh::io::Read>(reader: &mut R) -> Result<BigDecimal, borsh::io::Error> {
    let bigint_bytes: Vec<u8> = borsh::BorshDeserialize::deserialize_reader(reader)?;
    let scale: i64 = borsh::BorshDeserialize::deserialize_reader(reader)?;
    let bigint = BigInt::from_signed_bytes_le(&bigint_bytes);
    Ok(BigDecimal::from_bigint(bigint, scale))
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use bigdecimal::BigDecimal;
    use near_sdk::borsh;

    use super::WrappedBigDecimal;

    #[test]
    pub fn test() {
        let my_bigdecimal = BigDecimal::from_str("1.23456789").unwrap();
        let borshed = borsh::to_vec(&WrappedBigDecimal(my_bigdecimal.clone())).unwrap();
        let parsed: WrappedBigDecimal = borsh::from_slice(&borshed).unwrap();
        assert_eq!(my_bigdecimal, parsed.into());
    }
}
