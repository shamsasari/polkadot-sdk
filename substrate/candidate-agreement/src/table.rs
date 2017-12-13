// Copyright 2017 Parity Technologies (UK) Ltd.
// This file is part of Polkadot.

// Polkadot is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Polkadot is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Polkadot.  If not, see <http://www.gnu.org/licenses/>.

//! The statement table.
//!
//! This stores messages other validators issue about candidates.
//!
//! These messages are used to create a proposal submitted to a BFT consensus process.
//!
//! Proposals are formed of sets of candidates which have the requisite number of
//! validity and availability votes.
//!
//! Each parachain is associated with two sets of validators: those which can
//! propose and attest to validity of candidates, and those who can only attest
//! to availability.

use std::collections::{HashSet, HashMap};
use std::collections::hash_map::Entry;
use std::hash::Hash;
use std::fmt::Debug;

/// Statements circulated among peers.
#[derive(PartialEq, Eq, Debug)]
pub enum Statement<C: Context + ?Sized> {
	/// Broadcast by a validator to indicate that this is his candidate for
	/// inclusion.
	///
	/// Broadcasting two different candidate messages per round is not allowed.
	Candidate(C::Candidate),
	/// Broadcast by a validator to attest that the candidate with given digest
	/// is valid.
	Valid(C::Digest),
	/// Broadcast by a validator to attest that the auxiliary data for a candidate
	/// with given digest is available.
	Available(C::Digest),
	/// Broadcast by a validator to attest that the candidate with given digest
	/// is invalid.
	Invalid(C::Digest),
}

/// A signed statement.
#[derive(PartialEq, Eq, Debug)]
pub struct SignedStatement<C: Context + ?Sized> {
	/// The statement.
	pub statement: Statement<C>,
	/// The signature.
	pub signature: C::Signature,
}

/// Context for the statement table.
pub trait Context {
	/// A validator ID
	type ValidatorId: Hash + Eq + Clone + Debug;
	/// The digest (hash or other unique attribute) of a candidate.
	type Digest: Hash + Eq + Clone + Debug;
    /// Candidate type.
	type Candidate: Ord + Clone + Eq + Debug;
	/// The group ID type
	type GroupId: Hash + Eq + Clone + Debug;
	/// A signature type.
	type Signature: Clone + Eq + Debug;

	/// get the digest of a candidate.
	fn candidate_digest(&self, candidate: &Self::Candidate) -> Self::Digest;

	/// get the group of a candidate.
	fn candidate_group(&self, candidate: &Self::Candidate) -> Self::GroupId;

	/// Whether a validator is a member of a group.
	/// Members are meant to submit candidates and vote on validity.
	fn is_member_of(&self, validator: &Self::ValidatorId, group: &Self::GroupId) -> bool;

	/// Whether a validator is an availability guarantor of a group.
	/// Guarantors are meant to vote on availability for candidates submitted
	/// in a group.
	fn is_availability_guarantor_of(
		&self,
		validator: &Self::ValidatorId,
		group: &Self::GroupId,
	) -> bool;

	// recover signer of statement.
	fn statement_signer(
		&self,
		statement: &SignedStatement<Self>,
	) -> Option<Self::ValidatorId>;
}

/// Misbehavior: voting both ways on candidate validity.
#[derive(PartialEq, Eq, Debug)]
pub struct ValidityDoubleVote<C: Context> {
	/// The candidate digest
	pub digest: C::Digest,
	/// The signature on the true vote.
	pub t_signature: C::Signature,
	/// The signature on the false vote.
	pub f_signature: C::Signature,
}

/// Misbehavior: declaring multiple candidates.
#[derive(PartialEq, Eq, Debug)]
pub struct MultipleCandidates<C: Context> {
	/// The first candidate seen.
	pub first: (C::Candidate, C::Signature),
	/// The second candidate seen.
	pub second: (C::Candidate, C::Signature),
}

/// Misbehavior: submitted statement for wrong group.
#[derive(PartialEq, Eq, Debug)]
pub struct UnauthorizedStatement<C: Context> {
	/// A signed statement which was submitted without proper authority.
	pub statement: SignedStatement<C>,
}

/// Different kinds of misbehavior. All of these kinds of malicious misbehavior
/// are easily provable and extremely disincentivized.
#[derive(PartialEq, Eq, Debug)]
pub enum Misbehavior<C: Context> {
	/// Voted invalid and valid on validity.
	ValidityDoubleVote(ValidityDoubleVote<C>),
	/// Submitted multiple candidates.
	MultipleCandidates(MultipleCandidates<C>),
	/// Submitted a message withou
	UnauthorizedStatement(UnauthorizedStatement<C>),
}

// Votes on a specific candidate.
struct CandidateData<C: Context> {
	group_id: C::GroupId,
	candidate: C::Candidate,
	validity_votes: HashMap<C::ValidatorId, (bool, C::Signature)>,
	availability_votes: HashSet<C::ValidatorId>,
	indicated_bad_by: Vec<C::ValidatorId>,
}

/// Create a new, empty statement table.
pub fn create<C: Context>() -> Table<C> {
	Table {
		proposed_candidates: HashMap::default(),
		detected_misbehavior: HashMap::default(),
		candidate_votes: HashMap::default(),
	}
}

/// Stores votes
#[derive(Default)]
pub struct Table<C: Context> {
	proposed_candidates: HashMap<C::ValidatorId, (C::Digest, C::Signature)>,
	detected_misbehavior: HashMap<C::ValidatorId, Misbehavior<C>>,
	candidate_votes: HashMap<C::Digest, CandidateData<C>>,
}

impl<C: Context> Table<C> {
	/// Import a signed statement
	pub fn import_statement(&mut self, context: &C, statement: SignedStatement<C>) {
		let signer = match context.statement_signer(&statement) {
			None => return,
			Some(signer) => signer,
		};

		let maybe_misbehavior = match statement.statement {
			Statement::Candidate(candidate) => self.import_candidate(
				context,
				signer.clone(),
				candidate,
				statement.signature
			),
			Statement::Valid(digest) => self.validity_vote(
				context,
				signer.clone(),
				digest,
				true,
				statement.signature,
			),
			Statement::Invalid(digest) => self.validity_vote(
				context,
				signer.clone(),
				digest,
				false,
				statement.signature,
			),
			Statement::Available(digest) => self.availability_vote(
				context,
				signer.clone(),
				digest,
				statement.signature,
			)
		};

		if let Some(misbehavior) = maybe_misbehavior {
			// all misbehavior in agreement is provable and actively malicious.
			// punishments are not cumulative.
			self.detected_misbehavior.insert(signer, misbehavior);
		}
	}

	fn import_candidate(
		&mut self,
		context: &C,
		from: C::ValidatorId,
		candidate: C::Candidate,
		signature: C::Signature,
	) -> Option<Misbehavior<C>> {
		let group = context.candidate_group(&candidate);
		if !context.is_member_of(&from, &group) {
			return Some(Misbehavior::UnauthorizedStatement(UnauthorizedStatement {
				statement: SignedStatement {
					signature,
					statement: Statement::Candidate(candidate),
				},
			}));
		}

		// check that validator hasn't already specified another candidate.
		let digest = context.candidate_digest(&candidate);

		match self.proposed_candidates.entry(from.clone()) {
			Entry::Occupied(occ) => {
				// if digest is different, fetch candidate and
				// note misbehavior.
				let old_digest = &occ.get().0;
				if old_digest != &digest {
					let old_candidate = self.candidate_votes.get(old_digest)
						.expect("proposed digest implies existence of votes entry; qed")
						.candidate
						.clone();

					return Some(Misbehavior::MultipleCandidates(MultipleCandidates {
						first: (old_candidate, occ.get().1.clone()),
						second: (candidate, signature),
					}));
				}
			}
			Entry::Vacant(vacant) => {
				vacant.insert((digest.clone(), signature));

				// TODO: seed validity votes with issuer here?
				self.candidate_votes.entry(digest).or_insert_with(move || CandidateData {
					group_id: group,
					candidate: candidate,
					validity_votes: HashMap::new(),
					availability_votes: HashSet::new(),
					indicated_bad_by: Vec::new(),
				});
			}
		}

		None
	}

	fn validity_vote(
		&mut self,
		context: &C,
		from: C::ValidatorId,
		digest: C::Digest,
		valid: bool,
		signature: C::Signature,
	) -> Option<Misbehavior<C>> {
		let votes = match self.candidate_votes.get_mut(&digest) {
			None => return None, // TODO: queue up but don't get DoS'ed
			Some(votes) => votes,
		};

		// check that this validator actually can vote in this group.
		if !context.is_member_of(&from, &votes.group_id) {
			return Some(Misbehavior::UnauthorizedStatement(UnauthorizedStatement {
				statement: SignedStatement {
					signature: signature.clone(),
					statement: if valid {
						Statement::Valid(digest.clone())
					} else {
						Statement::Invalid(digest.clone())
					}
				}
			}));
		}

		// check for double votes.
		match votes.validity_votes.entry(from.clone()) {
			Entry::Occupied(occ) => {
				if occ.get().0 != valid {
					let (t_signature, f_signature) = if valid {
						(signature, occ.get().1.clone())
					} else {
						(occ.get().1.clone(), signature)
					};

					return Some(Misbehavior::ValidityDoubleVote(ValidityDoubleVote {
						digest: digest,
						t_signature,
						f_signature,
					}));
				}
			}
			Entry::Vacant(vacant) => {
				vacant.insert((valid, signature));
				votes.indicated_bad_by.push(from);
			}
		}

		None
	}

	fn availability_vote(
		&mut self,
		context: &C,
		from: C::ValidatorId,
		digest: C::Digest,
		signature: C::Signature,
	) -> Option<Misbehavior<C>> {
		let votes = match self.candidate_votes.get_mut(&digest) {
			None => return None, // TODO: queue up but don't get DoS'ed
			Some(votes) => votes,
		};

		// check that this validator actually can vote in this group.
		if !context.is_availability_guarantor_of(&from, &votes.group_id) {
			return Some(Misbehavior::UnauthorizedStatement(UnauthorizedStatement {
				statement: SignedStatement {
					signature: signature.clone(),
					statement: Statement::Available(digest),
				}
			}));
		}

		votes.availability_votes.insert(from);
		None
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use std::collections::HashMap;

	#[derive(Debug, Copy, Clone, Hash, PartialEq, Eq)]
	struct ValidatorId(usize);

	#[derive(Debug, Copy, Clone, Hash, PartialEq, Eq)]
	struct GroupId(usize);

	// group, body
	#[derive(Debug, Copy, Clone, Hash, PartialOrd, Ord, PartialEq, Eq)]
	struct Candidate(usize, usize);

	#[derive(Debug, Copy, Clone, Hash, PartialEq, Eq)]
	struct Signature(usize);

	#[derive(Debug, Copy, Clone, Hash, PartialEq, Eq)]
	struct Digest(usize);

	#[derive(Debug, PartialEq, Eq)]
	struct TestContext {
		// v -> (validity, availability)
		validators: HashMap<ValidatorId, (GroupId, GroupId)>
	}

	impl Context for TestContext {
		type ValidatorId = ValidatorId;
		type Digest = Digest;
		type Candidate = Candidate;
		type GroupId = GroupId;
		type Signature = Signature;

		fn candidate_digest(&self, candidate: &Candidate) -> Digest {
			Digest(candidate.1)
		}

		fn candidate_group(&self, candidate: &Candidate) -> GroupId {
			GroupId(candidate.0)
		}

		fn is_member_of(
			&self,
			validator: &ValidatorId,
			group: &GroupId
		) -> bool {
			self.validators.get(validator).map(|v| &v.0 == group).unwrap_or(false)
		}

		fn is_availability_guarantor_of(
			&self,
			validator: &ValidatorId,
			group: &GroupId
		) -> bool {
			self.validators.get(validator).map(|v| &v.1 == group).unwrap_or(false)
		}

		fn statement_signer(
			&self,
			statement: &SignedStatement<Self>,
		) -> Option<ValidatorId> {
			Some(ValidatorId(statement.signature.0))
		}
	}

	#[test]
	fn submitting_two_candidates_is_misbehavior() {
		let context = TestContext {
			validators: {
				let mut map = HashMap::new();
				map.insert(ValidatorId(1), (GroupId(2), GroupId(455)));
				map
			}
		};

		let mut table = create();
		let statement_a = SignedStatement {
			statement: Statement::Candidate(Candidate(2, 100)),
			signature: Signature(1),
		};

		let statement_b = SignedStatement {
			statement: Statement::Candidate(Candidate(2, 999)),
			signature: Signature(1),
		};

		table.import_statement(&context, statement_a);
		assert!(!table.detected_misbehavior.contains_key(&ValidatorId(1)));

		table.import_statement(&context, statement_b);
		assert_eq!(
			table.detected_misbehavior.get(&ValidatorId(1)).unwrap(),
			&Misbehavior::MultipleCandidates(MultipleCandidates {
				first: (Candidate(2, 100), Signature(1)),
				second: (Candidate(2, 999), Signature(1)),
			})
		);
	}
}
