#![feature(custom_attribute)]

use lazy_static::lazy_static;
use regex::Regex;
use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use wasm_bindgen::prelude::*;

use ndarray::prelude::*;
use ndarray::{Data, IxDyn};

#[derive(Debug)]
pub struct EinsumParse {
    operand_indices: Vec<String>,
    output_indices: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct Contraction {
    operand_indices: Vec<String>,
    output_indices: Vec<char>,
    summation_indices: Vec<char>,
}

pub type OutputSize = HashMap<char, usize>;

#[derive(Debug, Serialize)]
pub struct SizedContraction {
    contraction: Contraction,
    output_size: OutputSize,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct OperandSizes(Vec<Vec<usize>>);

#[derive(Debug, Serialize, Deserialize)]
pub struct FlattenedOperand<T> {
    shape: Vec<usize>,
    contents: Vec<T>,
}

pub fn generate_contraction(parse: &EinsumParse) -> Result<Contraction, &'static str> {
    let mut input_indices = HashMap::new();
    for c in parse.operand_indices.iter().flat_map(|s| s.chars()) {
        *input_indices.entry(c).or_insert(0) += 1;
    }

    let mut unique_indices = Vec::new();
    let mut duplicated_indices = Vec::new();
    for (&c, &n) in input_indices.iter() {
        let dst = if n > 1 {
            &mut duplicated_indices
        } else {
            &mut unique_indices
        };
        dst.push(c);
    }

    let requested_output_indices = match &parse.output_indices {
        Some(s) => s.chars().collect(),
        _ => {
            let mut o = unique_indices.clone();
            o.sort();
            o
        }
    };
    let mut distinct_output_indices = HashMap::new();
    for &c in requested_output_indices.iter() {
        *distinct_output_indices.entry(c).or_insert(0) += 1;
    }
    for (&c, &n) in distinct_output_indices.iter() {
        // No duplicates
        if n > 1 {
            return Err("Requested output has duplicate index");
        }

        // Must be in inputs
        if input_indices.get(&c).is_none() {
            return Err("Requested output contains an index not found in inputs");
        }
    }

    let output_indices = requested_output_indices;
    let mut summation_indices = Vec::new();
    for (&c, _) in input_indices.iter() {
        if distinct_output_indices.get(&c).is_none() {
            summation_indices.push(c);
        }
    }
    summation_indices.sort();

    Ok(Contraction {
        operand_indices: parse.operand_indices.clone(),
        output_indices: output_indices,
        summation_indices: summation_indices,
    })
}

pub fn parse_einsum_string(input_string: &str) -> Option<EinsumParse> {
    lazy_static! {
        // Unwhitespaced version:
        // ^([a-z]+)((?:,[a-z]+)*)(?:->([a-z]*))?$

        // Avoiding unwrap() here in a fruitless attempt to shrink
        // generated wasm size
        static ref RE: Regex = {
            match Regex::new(r"(?x)
            ^
            (?P<first_operand>[a-z]+)
            (?P<more_operands>(?:,[a-z]+)*)
            (?:->(?P<output>[a-z]*))?
            $
            ") {
                Ok(r) => r,
                _ => std::process::abort()
            }
        };
    }
    let captures = RE.captures(input_string)?;
    let mut operand_indices = Vec::new();
    let output_indices = captures.name("output").map(|s| String::from(s.as_str()));

    operand_indices.push(String::from(&captures["first_operand"]));
    for s in (&captures["more_operands"]).split(',').skip(1) {
        operand_indices.push(String::from(s));
    }

    Some(EinsumParse {
        operand_indices: operand_indices,
        output_indices: output_indices,
    })
}

pub fn validate(input_string: &str) -> Result<Contraction, &'static str> {
    let p = parse_einsum_string(input_string).ok_or("Invalid string")?;
    generate_contraction(&p)
}

pub trait ArrayLike<A> {
    fn into_dyn_view(&self) -> ArrayView<A, IxDyn>;
}

impl<A, S, D> ArrayLike<A> for ArrayBase<S, D>
where
    S: Data<Elem = A>,
    D: Dimension,
{
    fn into_dyn_view(&self) -> ArrayView<A, IxDyn> {
        self.view().into_dyn()
    }
}

pub fn get_output_size_from_shapes(
    contraction: &Contraction,
    operand_shapes: &Vec<Vec<usize>>,
) -> Result<OutputSize, &'static str> {
    // Check that len(operand_indices) == len(operands)
    if contraction.operand_indices.len() != operand_shapes.len() {
        return Err("number of operands in contraction does not match number of operands supplied");
    }

    let mut index_lengths: OutputSize = HashMap::new();

    for (indices, operand_shape) in contraction.operand_indices.iter().zip(operand_shapes) {
        // Check that len(operand_indices[i]) == len(operands[i].shape())
        if indices.chars().count() != operand_shape.len() {
            return Err(
                "number of indices in one or more operands does not match dimensions of operand",
            );
        }

        // Check that whenever there are multiple copies of an index,
        // operands[i].shape()[m] == operands[j].shape()[n]
        for (c, &n) in indices.chars().zip(operand_shape) {
            let existing_n = index_lengths.entry(c).or_insert(n);
            if *existing_n != n {
                return Err("repeated index with different size");
            }
        }
    }

    Ok(index_lengths)
}

pub fn get_operand_shapes<A>(operands: &[&dyn ArrayLike<A>]) -> Vec<Vec<usize>> {
    operands
        .iter()
        .map(|operand| Vec::from(operand.into_dyn_view().shape()))
        .collect()
}

pub fn get_output_size<A>(
    contraction: &Contraction,
    operands: &[&dyn ArrayLike<A>],
) -> Result<HashMap<char, usize>, &'static str> {
    get_output_size_from_shapes(contraction, &get_operand_shapes(operands))
}

pub fn validate_and_size_from_shapes(
    input_string: &str,
    operand_shapes: &Vec<Vec<usize>>,
) -> Result<SizedContraction, &'static str> {
    let contraction = validate(input_string)?;
    let output_size = get_output_size_from_shapes(&contraction, operand_shapes)?;

    Ok(SizedContraction {
        contraction,
        output_size,
    })
}

pub fn validate_and_size_from_shapes_as_string(
    input_string: &str,
    operand_shapes_as_str: &str,
) -> Result<SizedContraction, &'static str> {
    match serde_json::from_str::<OperandSizes>(&operand_shapes_as_str) {
        Err(_) => Err("Error parsing operand shapes into Vec<Vec<usize>>"),
        Ok(OperandSizes(operand_shapes)) => {
            validate_and_size_from_shapes(input_string, &operand_shapes)
        }
    }
}

pub fn validate_and_size<A>(
    input_string: &str,
    operands: &[&dyn ArrayLike<A>],
) -> Result<SizedContraction, &'static str> {
    validate_and_size_from_shapes(input_string, &get_operand_shapes(operands))
}

fn make_index(indices: &str, bindings: &HashMap<char, usize>) -> IxDyn {
    ////// PYTHON: ///////////////////
    // def make_tuple(
    //     indices,
    //     bindings,
    // ):
    //     return tuple([bindings[x] for x in indices])
    //////////////////////////////////
    let mut v: Vec<usize> = Vec::new();
    for i in indices.chars() {
        v.push(bindings[&i])
    }
    IxDyn(&v)
}

pub trait Einsummable:
    Copy
    + std::ops::Add<Output = Self>
    + num_traits::identities::Zero
    + std::ops::Mul<Output = Self>
    + num_traits::identities::One
{
}
impl<A> Einsummable for A where
    A: Copy
        + std::ops::Add<Output = A>
        + num_traits::identities::Zero
        + std::ops::Mul<Output = A>
        + num_traits::identities::One
{
}

fn partial_einsum_inner_loop<A: Einsummable>(
    operands: &[&ArrayViewD<A>],
    operand_indices: &Vec<String>,
    bound_indices: &HashMap<char, usize>,
    axis_lengths: &HashMap<char, usize>,
    free_summation_indices: &[char],
) -> A {
    ////// PYTHON: ///////////////////
    // def partial_einsum_inner_loop(...):
    //     if len(free_summation_indices) == 0:
    //         return np.product([
    //             operand[make_tuple(indices, bound_indices)]
    //             for (operand, indices) in zip(operands, operand_indices)
    //         ])
    //     else:
    //         next_index = free_summation_indices[0]
    //         remaining_indices = free_summation_indices[1:]
    //         partial_sum = 0
    //         for i in range(axis_lengths[next_index]):
    //             partial_sum += partial_einsum_inner_loop(
    //                 operands=operands,
    //                 operand_indices=operand_indices,
    //                 bound_indices={**bound_indices, **{next_index: i}},
    //                 axis_lengths=axis_lengths,
    //                 free_summation_indices=remaining_indices
    //             )
    //         return partial_sum
    //////////////////////////////////
    if free_summation_indices.len() == 0 {
        let mut p = num_traits::identities::one::<A>();
        for (operand, indices) in operands.iter().zip(operand_indices) {
            let index = make_index(&indices, bound_indices);
            p = p * operand[index];
        }
        p
    } else {
        let next_index = free_summation_indices[0];
        let remaining_indices = &free_summation_indices[1..];
        let mut s = num_traits::identities::zero::<A>();
        for i in 0..axis_lengths[&next_index] {
            let mut new_bound_indices = bound_indices.clone();
            new_bound_indices.insert(next_index, i);

            s = s + partial_einsum_inner_loop(
                operands,
                operand_indices,
                &new_bound_indices,
                axis_lengths,
                remaining_indices,
            )
        }
        s
    }
}

pub fn partial_einsum_outer_loop<A: Einsummable>(
    operands: &[&ArrayViewD<A>],
    operand_indices: &Vec<String>,
    bound_indices: &HashMap<char, usize>,
    free_output_indices: &[char],
    axis_lengths: &HashMap<char, usize>,
    summation_indices: &[char],
) -> ArrayD<A> {
    ////// PYTHON: ///////////////////
    // def partial_einsum_outer_loop(...):
    //     if len(free_output_indices) == 0:
    //         return partial_einsum_inner_loop(
    //             operands=operands,
    //             operand_indices=operand_indices,
    //             bound_indices=bound_indices,
    //             axis_lengths=axis_lengths,
    //             free_summation_indices=summation_indices
    //         )
    //     else:
    //         next_index = free_output_indices[0]
    //         remaining_indices = free_output_indices[1:]
    //         return np.array([
    //             partial_einsum_outer_loop(
    //                 operands=operands,
    //                 operand_indices=operand_indices,
    //                 bound_indices={**bound_indices, **{next_index: i}},
    //                 free_output_indices=remaining_indices,
    //                 axis_lengths=axis_lengths,
    //                 summation_indices=summation_indices
    //             )
    //             for i in range(axis_lengths[next_index])
    //         ])
    //////////////////////////////////
    if free_output_indices.len() == 0 {
        arr0(partial_einsum_inner_loop(
            operands,
            operand_indices,
            bound_indices,
            axis_lengths,
            summation_indices,
        ))
        .into_dyn()
    } else {
        let output_shapes: Vec<_> = free_output_indices
            .iter()
            .map(|c| axis_lengths[c])
            .collect();
        let output_shape = IxDyn(&output_shapes);
        let mut result: ArrayD<A> = Array::zeros(output_shape);
        let next_index = free_output_indices[0];
        let remaining_indices = &free_output_indices[1..];

        for i in 0..axis_lengths[&next_index] {
            let mut new_bound_indices = bound_indices.clone();
            new_bound_indices.insert(next_index, i);
            let slice = partial_einsum_outer_loop(
                operands,
                operand_indices,
                &new_bound_indices,
                remaining_indices,
                axis_lengths,
                summation_indices,
            );
            let mut mutable_subslice = result.index_axis_mut(Axis(0), i);
            ndarray::Zip::from(&mut mutable_subslice)
                .and(&slice)
                .apply(|m, s| {
                    *m = *s;
                });
        }

        result
    }
}

pub fn slow_einsum_given_sized_contraction<A: Einsummable>(
    sized_contraction: &SizedContraction,
    operands: &[&dyn ArrayLike<A>],
) -> ArrayD<A> {
    ////// PYTHON: ///////////////////
    // def my_einsum(
    //     contraction,
    //     operands,
    //     axis_lengths,
    // ):
    //     return partial_einsum_outer_loop(
    //         operands=operands,
    //         operand_indices=contraction["operand_indices"],
    //         bound_indices={},
    //         free_output_indices=contraction["output_indices"],
    //         axis_lengths=axis_lengths,
    //         summation_indices=contraction["summation_indices"]
    //     )
    //////////////////////////////////
    let dyn_operands: Vec<ArrayViewD<A>> = operands.iter().map(|x| x.into_dyn_view()).collect();
    let operand_refs: Vec<&ArrayViewD<A>> = dyn_operands.iter().map(|x| x).collect();
    let bound_indices: HashMap<char, usize> = HashMap::new();

    partial_einsum_outer_loop(
        &operand_refs,
        &sized_contraction.contraction.operand_indices,
        &bound_indices,
        &sized_contraction.contraction.output_indices,
        &sized_contraction.output_size,
        &sized_contraction.contraction.summation_indices,
    )
}

pub fn slow_einsum<A: Einsummable>(
    input_string: &str,
    operands: &[&dyn ArrayLike<A>],
) -> Result<ArrayD<A>, &'static str> {
    let sized_contraction = validate_and_size(input_string, operands)?;
    Ok(slow_einsum_given_sized_contraction(
        &sized_contraction,
        operands,
    ))
}

// pub fn slow_einsum_with_flattened_operands<A>(
//     input_string: &str,
//     flattened_operands: &[&FlattenedOperand<A>],
// ) -> Result<ArrayD<A>, &'static str>
// {
//     let sized_contraction = validate_and_size(input_string, operands)?;
//     Ok(slow_einsum_given_sized_contraction(
//         &sized_contraction,
//         operands,
//     ))
// }

////////////////////////// WASM stuff below here ///////////////////////
#[derive(Debug, Serialize)]
pub struct ContractionResult(Result<Contraction, &'static str>);

#[wasm_bindgen(js_name = validateAsJson)]
pub fn validate_as_json(input_string: &str) -> String {
    match serde_json::to_string(&ContractionResult(validate(input_string))) {
        Ok(s) => s,
        _ => String::from("{\"Err\": \"Serialization Error\"}"),
    }
}

#[derive(Debug, Serialize)]
pub struct SizedContractionResult(Result<SizedContraction, &'static str>);

#[wasm_bindgen(js_name = validateAndSizeFromShapesAsStringAsJson)]
pub fn validate_and_size_from_shapes_as_string_as_json(
    input_string: &str,
    operand_shapes: &str,
) -> String {
    match serde_json::to_string(&SizedContractionResult(
        validate_and_size_from_shapes_as_string(input_string, operand_shapes),
    )) {
        Ok(s) => s,
        _ => String::from("{\"Err\": \"Serialization Error\"}"),
    }
}
