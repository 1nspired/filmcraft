//! Current sequence-service fixture. Kept separately so rebuilding this example
//! cannot replace the qualified inspection executable used by earlier gates.
#[path = "agent_inspection_fixture.rs"]
mod shared;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    shared::main()
}
