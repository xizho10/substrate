// This file is part of Substrate.

// Copyright (C) 2017-2020 Parity Technologies (UK) Ltd.
// SPDX-License-Identifier: Apache-2.0

// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// 	http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::{Trait, Module};
use codec::{Encode, Decode};
use sp_runtime::{
	traits::{SignedExtension, DispatchInfoOf, Dispatchable, PostDispatchInfoOf, Printable},
	transaction_validity::{
		ValidTransaction, TransactionValidityError, InvalidTransaction, TransactionValidity,
		TransactionPriority,
	},
	DispatchResult,
};
use frame_support::{
	traits::{Get},
	weights::{PostDispatchInfo, DispatchInfo, DispatchClass, priority::FrameTransactionPriority},
	StorageValue,
};

/// Block resource (weight) limit check.
#[derive(Encode, Decode, Clone, Eq, PartialEq, Default)]
pub struct CheckWeight<T: Trait + Send + Sync>(sp_std::marker::PhantomData<T>);

impl<T: Trait + Send + Sync> CheckWeight<T> where
	T::Call: Dispatchable<Info=DispatchInfo, PostInfo=PostDispatchInfo>
{
	/// Checks if the current extrinsic does not exceed the maximum weight a single extrinsic
	/// with given `DispatchClass` can have.
	fn check_extrinsic_weight(
		info: &DispatchInfoOf<T::Call>,
	) -> Result<(), TransactionValidityError> {
		let max = T::BlockWeights::get().get(info.class).max_extrinsic;
		match max {
			Some(max) if info.weight > max => {
				Err(InvalidTransaction::ExhaustsResources.into())
			},
			_ => Ok(()),
		}
	}

	/// Checks if the current extrinsic can fit into the block with respect to block weight limits.
	///
	/// Upon successes, it returns the new block weight as a `Result`.
	fn check_block_weight(
		info: &DispatchInfoOf<T::Call>,
	) -> Result<crate::ConsumedWeight, TransactionValidityError> {
		let maximum_weight = T::BlockWeights::get();
		let mut all_weight = Module::<T>::block_weight();
		let extrinsic_weight = info.weight.saturating_add(maximum_weight.get(info.class).base_extrinsic);

		if let Some(max) = maximum_weight.get(info.class).max_total {
			all_weight.checked_add(extrinsic_weight, info.class)
				.map_err(|_| InvalidTransaction::ExhaustsResources)?;
			let per_class = *all_weight.get(info.class);

			// Class allowance exceeded
			if per_class > max {
				return Err(InvalidTransaction::ExhaustsResources.into());
			}

			// Total block weight exceeded.
			if all_weight.total() > maximum_weight.max_block {
				// Check if we can use reserved pool though.
				match maximum_weight.get(info.class).reserved {
					Some(reserved) if per_class > reserved => {
						return Err(InvalidTransaction::ExhaustsResources.into());
					}
					_ => {},
				}
			}
		} else {
			all_weight.add(extrinsic_weight, info.class);
		}

		Ok(all_weight)
	}

	/// Checks if the current extrinsic can fit into the block with respect to block length limits.
	///
	/// Upon successes, it returns the new block length as a `Result`.
	fn check_block_length(
		info: &DispatchInfoOf<T::Call>,
		len: usize,
	) -> Result<u32, TransactionValidityError> {
		let length_limit = T::BlockLength::get();
		let current_len = Module::<T>::all_extrinsics_len();
		let added_len = len as u32;
		let next_len = current_len.saturating_add(added_len);
		if next_len > *length_limit.max.get(info.class) {
			Err(InvalidTransaction::ExhaustsResources.into())
		} else {
			Ok(next_len)
		}
	}

	/// Get the priority of an extrinsic denoted by `info`.
	///
	/// Operational transaction will be given a fixed initial amount to be fairly distinguished from
	/// the normal ones.
	fn get_priority(info: &DispatchInfoOf<T::Call>) -> TransactionPriority {
		match info.class {
			// Normal transaction.
			DispatchClass::Normal =>
				FrameTransactionPriority::Normal(info.weight.into()).into(),
			// Don't use up the whole priority space, to allow things like `tip` to be taken into
			// account as well.
			DispatchClass::Operational =>
				FrameTransactionPriority::Operational(info.weight.into()).into(),
			// Mandatory extrinsics are only for inherents; never transactions.
			DispatchClass::Mandatory => TransactionPriority::min_value(),
		}
	}

	/// Creates new `SignedExtension` to check weight of the extrinsic.
	pub fn new() -> Self {
		Self(Default::default())
	}

	/// Do the pre-dispatch checks. This can be applied to both signed and unsigned.
	///
	/// It checks and notes the new weight and length.
	pub fn do_pre_dispatch(
		info: &DispatchInfoOf<T::Call>,
		len: usize,
	) -> Result<(), TransactionValidityError> {
		let next_len = Self::check_block_length(info, len)?;
		let next_weight = Self::check_block_weight(info)?;
		Self::check_extrinsic_weight(info)?;

		crate::AllExtrinsicsLen::put(next_len);
		crate::BlockWeight::put(next_weight);
		Ok(())
	}

	/// Do the validate checks. This can be applied to both signed and unsigned.
	///
	/// It only checks that the block weight and length limit will not exceed.
	pub fn do_validate(
		info: &DispatchInfoOf<T::Call>,
		len: usize,
	) -> TransactionValidity {
		// ignore the next length. If they return `Ok`, then it is below the limit.
		let _ = Self::check_block_length(info, len)?;
		// during validation we skip block limit check. Since the `validate_transaction`
		// call runs on an empty block anyway, by this we prevent `on_initialize` weight
		// consumption from causing false negatives.
		Self::check_extrinsic_weight(info)?;

		Ok(ValidTransaction { priority: Self::get_priority(info), ..Default::default() })
	}
}

impl<T: Trait + Send + Sync> SignedExtension for CheckWeight<T> where
	T::Call: Dispatchable<Info=DispatchInfo, PostInfo=PostDispatchInfo>
{
	type AccountId = T::AccountId;
	type Call = T::Call;
	type AdditionalSigned = ();
	type Pre = ();
	const IDENTIFIER: &'static str = "CheckWeight";

	fn additional_signed(&self) -> sp_std::result::Result<(), TransactionValidityError> { Ok(()) }

	fn pre_dispatch(
		self,
		_who: &Self::AccountId,
		_call: &Self::Call,
		info: &DispatchInfoOf<Self::Call>,
		len: usize,
	) -> Result<(), TransactionValidityError> {
		if info.class == DispatchClass::Mandatory {
			Err(InvalidTransaction::MandatoryDispatch)?
		}
		Self::do_pre_dispatch(info, len)
	}

	fn validate(
		&self,
		_who: &Self::AccountId,
		_call: &Self::Call,
		info: &DispatchInfoOf<Self::Call>,
		len: usize,
	) -> TransactionValidity {
		if info.class == DispatchClass::Mandatory {
			Err(InvalidTransaction::MandatoryDispatch)?
		}
		Self::do_validate(info, len)
	}

	fn pre_dispatch_unsigned(
		_call: &Self::Call,
		info: &DispatchInfoOf<Self::Call>,
		len: usize,
	) -> Result<(), TransactionValidityError> {
		Self::do_pre_dispatch(info, len)
	}

	fn validate_unsigned(
		_call: &Self::Call,
		info: &DispatchInfoOf<Self::Call>,
		len: usize,
	) -> TransactionValidity {
		Self::do_validate(info, len)
	}

	fn post_dispatch(
		_pre: Self::Pre,
		info: &DispatchInfoOf<Self::Call>,
		post_info: &PostDispatchInfoOf<Self::Call>,
		_len: usize,
		result: &DispatchResult,
	) -> Result<(), TransactionValidityError> {
		// Since mandatory dispatched do not get validated for being overweight, we are sensitive
		// to them actually being useful. Block producers are thus not allowed to include mandatory
		// extrinsics that result in error.
		if let (DispatchClass::Mandatory, Err(e)) = (info.class, result) {
			"Bad mandatory".print();
			e.print();

			Err(InvalidTransaction::BadMandatory)?
		}

		let unspent = post_info.calc_unspent(info);
		if unspent > 0 {
			crate::BlockWeight::mutate(|current_weight| {
				current_weight.sub(unspent, info.class);
			})
		}

		Ok(())
	}
}

impl<T: Trait + Send + Sync> sp_std::fmt::Debug for CheckWeight<T> {
	#[cfg(feature = "std")]
	fn fmt(&self, f: &mut sp_std::fmt::Formatter) -> sp_std::fmt::Result {
		write!(f, "CheckWeight")
	}

	#[cfg(not(feature = "std"))]
	fn fmt(&self, _: &mut sp_std::fmt::Formatter) -> sp_std::fmt::Result {
		Ok(())
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::{BlockWeight, AllExtrinsicsLen};
	use crate::mock::{Test, CALL, new_test_ext, System};
	use sp_std::marker::PhantomData;
	use frame_support::{assert_ok, assert_noop};
	use frame_support::weights::{Weight, Pays};

	fn block_weights() -> crate::limits::BlockWeights {
		<Test as crate::Trait>::BlockWeights::get()
	}

	fn normal_weight_limit() -> Weight {
		block_weights().get(DispatchClass::Normal).max_total
			.unwrap_or_else(|| block_weights().max_block)
	}

	fn block_weight_limit() -> Weight {
		block_weights().max_block
	}

	fn normal_length_limit() -> u32 {
		*<Test as Trait>::BlockLength::get().max.get(DispatchClass::Normal)
	}

	#[test]
	fn mandatory_extrinsic_doesnt_care_about_limits() {
		fn check(call: impl FnOnce(&DispatchInfo, usize)) {
			new_test_ext().execute_with(|| {
				let max = DispatchInfo {
					weight: Weight::max_value(),
					class: DispatchClass::Mandatory,
					..Default::default()
				};
				let len = 0_usize;

				call(&max, len);
			});
		}

		check(|max, len| {
			assert_ok!(CheckWeight::<Test>::do_pre_dispatch(max, len));
			assert_eq!(System::block_weight().total(), Weight::max_value());
			assert!(System::block_weight().total() > block_weight_limit());
		});
		check(|max, len| {
			assert_ok!(CheckWeight::<Test>::do_validate(max, len));
		});
	}

	#[test]
	fn normal_extrinsic_limited_by_maximum_extrinsic_weight() {
		new_test_ext().execute_with(|| {
			let max = DispatchInfo {
				weight: block_weights().get(DispatchClass::Normal).max_extrinsic.unwrap() + 1,
				class: DispatchClass::Normal,
				..Default::default()
			};
			let len = 0_usize;

			assert_noop!(
				CheckWeight::<Test>::do_validate(&max, len),
				InvalidTransaction::ExhaustsResources
			);
		});
	}

	#[test]
	fn operational_extrinsic_limited_by_operational_space_limit() {
		new_test_ext().execute_with(|| {
			let weights = block_weights();
			let operational_limit = weights.get(DispatchClass::Operational).max_total
				.unwrap_or_else(|| weights.max_block);
			let base_weight = weights.get(DispatchClass::Normal).base_extrinsic;

			let weight = operational_limit - base_weight;
			let okay = DispatchInfo {
				weight,
				class: DispatchClass::Operational,
				..Default::default()
			};
			let max = DispatchInfo {
				weight: weight + 1,
				class: DispatchClass::Operational,
				..Default::default()
			};
			let len = 0_usize;

			assert_eq!(
				CheckWeight::<Test>::do_validate(&okay, len),
				Ok(ValidTransaction {
					priority: CheckWeight::<Test>::get_priority(&okay),
					..Default::default()
				})
			);
			assert_noop!(
				CheckWeight::<Test>::do_validate(&max, len),
				InvalidTransaction::ExhaustsResources
			);
		});
	}

	#[test]
	fn register_extra_weight_unchecked_doesnt_care_about_limits() {
		new_test_ext().execute_with(|| {
			System::register_extra_weight_unchecked(Weight::max_value(), DispatchClass::Normal);
			assert_eq!(System::block_weight().total(), Weight::max_value());
			assert!(System::block_weight().total() > block_weight_limit());
		});
	}

	#[test]
	fn full_block_with_normal_and_operational() {
		new_test_ext().execute_with(|| {
			// Max block is 1024
			// Max normal is 768 (75%)
			// 10 is taken for block execution weight
			// So normal extrinsic can be 758 weight (-5 for base extrinsic weight)
			// And Operational can be 256 to produce a full block (-5 for base)
			let max_normal = DispatchInfo { weight: 753, ..Default::default() };
			let rest_operational = DispatchInfo { weight: 251, class: DispatchClass::Operational, ..Default::default() };

			let len = 0_usize;

			assert_ok!(CheckWeight::<Test>::do_pre_dispatch(&max_normal, len));
			assert_eq!(System::block_weight().total(), 768);
			assert_ok!(CheckWeight::<Test>::do_pre_dispatch(&rest_operational, len));
			assert_eq!(block_weight_limit(), 1024);
			assert_eq!(System::block_weight().total(), block_weight_limit());
			// Checking single extrinsic should not take current block weight into account.
			assert_eq!(CheckWeight::<Test>::check_extrinsic_weight(&rest_operational), Ok(()));
		});
	}

	#[test]
	fn dispatch_order_does_not_effect_weight_logic() {
		new_test_ext().execute_with(|| {
			// We switch the order of `full_block_with_normal_and_operational`
			let max_normal = DispatchInfo { weight: 753, ..Default::default() };
			let rest_operational = DispatchInfo { weight: 251, class: DispatchClass::Operational, ..Default::default() };

			let len = 0_usize;

			assert_ok!(CheckWeight::<Test>::do_pre_dispatch(&rest_operational, len));
			// Extra 15 here from block execution + base extrinsic weight
			assert_eq!(System::block_weight().total(), 266);
			assert_ok!(CheckWeight::<Test>::do_pre_dispatch(&max_normal, len));
			assert_eq!(block_weight_limit(), 1024);
			assert_eq!(System::block_weight().total(), block_weight_limit());
		});
	}

	#[test]
	fn operational_works_on_full_block() {
		new_test_ext().execute_with(|| {
			// An on_initialize takes up the whole block! (Every time!)
			System::register_extra_weight_unchecked(Weight::max_value(), DispatchClass::Mandatory);
			let dispatch_normal = DispatchInfo { weight: 251, class: DispatchClass::Normal, ..Default::default() };
			let dispatch_operational = DispatchInfo { weight: 251, class: DispatchClass::Operational, ..Default::default() };
			let len = 0_usize;

			assert_noop!(
				CheckWeight::<Test>::do_pre_dispatch(&dispatch_normal, len),
				InvalidTransaction::ExhaustsResources
			);
			// Thank goodness we can still do an operational transaction to possibly save the blockchain.
			assert_ok!(CheckWeight::<Test>::do_pre_dispatch(&dispatch_operational, len));
			// Not too much though
			assert_noop!(
				CheckWeight::<Test>::do_pre_dispatch(&dispatch_operational, len),
				InvalidTransaction::ExhaustsResources
			);
			// Even with full block, validity of single transaction should be correct.
			assert_eq!(CheckWeight::<Test>::check_extrinsic_weight(&dispatch_operational), Ok(()));
		});
	}

	#[test]
	fn signed_ext_check_weight_works_operational_tx() {
		new_test_ext().execute_with(|| {
			let normal = DispatchInfo { weight: 100, ..Default::default() };
			let op = DispatchInfo { weight: 100, class: DispatchClass::Operational, pays_fee: Pays::Yes };
			let len = 0_usize;
			let normal_limit = normal_weight_limit();

			// given almost full block
			BlockWeight::mutate(|current_weight| {
				current_weight.set(normal_limit, DispatchClass::Normal)
			});
			// will not fit.
			assert!(CheckWeight::<Test>(PhantomData).pre_dispatch(&1, CALL, &normal, len).is_err());
			// will fit.
			assert!(CheckWeight::<Test>(PhantomData).pre_dispatch(&1, CALL, &op, len).is_ok());

			// likewise for length limit.
			let len = 100_usize;
			AllExtrinsicsLen::put(normal_length_limit());
			assert!(CheckWeight::<Test>(PhantomData).pre_dispatch(&1, CALL, &normal, len).is_err());
			assert!(CheckWeight::<Test>(PhantomData).pre_dispatch(&1, CALL, &op, len).is_ok());
		})
	}

	#[test]
	fn signed_ext_check_weight_works() {
		new_test_ext().execute_with(|| {
			let normal = DispatchInfo { weight: 100, class: DispatchClass::Normal, pays_fee: Pays::Yes };
			let op = DispatchInfo { weight: 100, class: DispatchClass::Operational, pays_fee: Pays::Yes };
			let len = 0_usize;

			let priority = CheckWeight::<Test>(PhantomData)
				.validate(&1, CALL, &normal, len)
				.unwrap()
				.priority;
			assert_eq!(priority, 100);

			let priority = CheckWeight::<Test>(PhantomData)
				.validate(&1, CALL, &op, len)
				.unwrap()
				.priority;
			assert_eq!(priority, frame_support::weights::priority::LIMIT + 100);
		})
	}

	#[test]
	fn signed_ext_check_weight_block_size_works() {
		new_test_ext().execute_with(|| {
			let normal = DispatchInfo::default();
			let normal_limit = normal_weight_limit() as usize;
			let reset_check_weight = |tx, s, f| {
				AllExtrinsicsLen::put(0);
				let r = CheckWeight::<Test>(PhantomData).pre_dispatch(&1, CALL, tx, s);
				if f { assert!(r.is_err()) } else { assert!(r.is_ok()) }
			};

			reset_check_weight(&normal, normal_limit - 1, false);
			reset_check_weight(&normal, normal_limit, false);
			reset_check_weight(&normal, normal_limit + 1, true);

			// Operational ones don't have this limit.
			let op = DispatchInfo { weight: 0, class: DispatchClass::Operational, pays_fee: Pays::Yes };
			reset_check_weight(&op, normal_limit, false);
			reset_check_weight(&op, normal_limit + 100, false);
			reset_check_weight(&op, 1024, false);
			reset_check_weight(&op, 1025, true);
		})
	}


	#[test]
	fn signed_ext_check_weight_works_normal_tx() {
		new_test_ext().execute_with(|| {
			let normal_limit = normal_weight_limit();
			let small = DispatchInfo { weight: 100, ..Default::default() };
			let base_extrinsic = block_weights().get(DispatchClass::Normal).base_extrinsic;
			let medium = DispatchInfo {
				weight: normal_limit - base_extrinsic,
				..Default::default()
			};
			let big = DispatchInfo {
				weight: normal_limit - base_extrinsic + 1,
				..Default::default()
			};
			let len = 0_usize;

			let reset_check_weight = |i, f, s| {
				BlockWeight::mutate(|current_weight| {
					current_weight.set(s, DispatchClass::Normal)
				});
				let r = CheckWeight::<Test>(PhantomData).pre_dispatch(&1, CALL, i, len);
				if f { assert!(r.is_err()) } else { assert!(r.is_ok()) }
			};

			reset_check_weight(&small, false, 0);
			reset_check_weight(&medium, false, 0);
			reset_check_weight(&big, true, 1);
		})
	}

	#[test]
	fn signed_ext_check_weight_refund_works() {
		new_test_ext().execute_with(|| {
			// This is half of the max block weight
			let info = DispatchInfo { weight: 512, ..Default::default() };
			let post_info = PostDispatchInfo {
				actual_weight: Some(128),
				pays_fee: Default::default(),
			};
			let len = 0_usize;
			let base_extrinsic = block_weights().get(DispatchClass::Normal).base_extrinsic;

			// We allow 75% for normal transaction, so we put 25% - extrinsic base weight
			BlockWeight::mutate(|current_weight| {
				current_weight.set(0, DispatchClass::Mandatory);
				current_weight.set(256 - base_extrinsic, DispatchClass::Normal);
			});

			let pre = CheckWeight::<Test>(PhantomData).pre_dispatch(&1, CALL, &info, len).unwrap();
			assert_eq!(BlockWeight::get().total(), info.weight + 256);

			assert!(
				CheckWeight::<Test>::post_dispatch(pre, &info, &post_info, len, &Ok(()))
				.is_ok()
			);
			assert_eq!(
				BlockWeight::get().total(),
				post_info.actual_weight.unwrap() + 256,
			);
		})
	}

	#[test]
	fn signed_ext_check_weight_actual_weight_higher_than_max_is_capped() {
		new_test_ext().execute_with(|| {
			let info = DispatchInfo { weight: 512, ..Default::default() };
			let post_info = PostDispatchInfo {
				actual_weight: Some(700),
				pays_fee: Default::default(),
			};
			let len = 0_usize;

			BlockWeight::mutate(|current_weight| {
				current_weight.set(0, DispatchClass::Mandatory);
				current_weight.set(128, DispatchClass::Normal);
			});

			let pre = CheckWeight::<Test>(PhantomData).pre_dispatch(&1, CALL, &info, len).unwrap();
			assert_eq!(
				BlockWeight::get().total(),
				info.weight + 128 + block_weights().get(DispatchClass::Normal).base_extrinsic,
			);

			assert!(
				CheckWeight::<Test>::post_dispatch(pre, &info, &post_info, len, &Ok(()))
				.is_ok()
			);
			assert_eq!(
				BlockWeight::get().total(),
				info.weight + 128 + block_weights().get(DispatchClass::Normal).base_extrinsic,
			);
		})
	}

	#[test]
	fn zero_weight_extrinsic_still_has_base_weight() {
		new_test_ext().execute_with(|| {
			let weights = block_weights();
			let free = DispatchInfo { weight: 0, ..Default::default() };
			let len = 0_usize;

			// Initial weight from `weights.base_block`
			assert_eq!(
				System::block_weight().total(),
				weights.base_block
			);
			let r = CheckWeight::<Test>(PhantomData).pre_dispatch(&1, CALL, &free, len);
			assert!(r.is_ok());
			assert_eq!(
				System::block_weight().total(),
				weights.get(DispatchClass::Normal).base_extrinsic + weights.base_block
			);
		})
	}

	#[test]
	fn normal_and_mandatory_tracked_separately() {
		new_test_ext().execute_with(|| {
			// Max block is 1024
			// Max normal is 768 (75%)
			// Max mandatory is unlimited
			let max_normal = DispatchInfo { weight: 753, ..Default::default() };
			let mandatory = DispatchInfo { weight: 1019, class: DispatchClass::Mandatory, ..Default::default() };

			let len = 0_usize;

			assert_ok!(CheckWeight::<Test>::do_pre_dispatch(&max_normal, len));
			assert_eq!(System::block_weight().total(), 768);
			assert_ok!(CheckWeight::<Test>::do_pre_dispatch(&mandatory, len));
			assert_eq!(block_weight_limit(), 1024);
			assert_eq!(System::block_weight().total(), 1024 + 758);
			assert_eq!(CheckWeight::<Test>::check_extrinsic_weight(&mandatory), Ok(()));
		});
	}
}
