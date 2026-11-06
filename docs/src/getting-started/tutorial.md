# Tutorial

A typical usage of midenup and miden might look like the following:

1. midenup has been downloaded and correctly configured following the instructions of the [Installation](installation.md) page or [README](https://github.com/0xMiden/midenup)
2. The toolchain deployed to mainnet can then be installed:

   ```shell title=">_ Terminal"
   midenup install mainnet
   ```

3. With the toolchain now installed, the installed components can be inspected with the following command:

   ```shell title=">_ Terminal"
   miden help toolchain
   ```

4. To start, create a directory for the project and write a Miden Assembly program in it, e.g. `program.masm`:
   ```shell title=">_ Terminal"
   mkdir miden_project && cd miden_project
   ```
   ```masm title="program.masm"
   begin
       add
   end
   ```
   Along with the inputs it reads from the operand stack, in `program.inputs`:
   ```json title="program.inputs"
   {
       "operand_stack": ["3", "4"]
   }
   ```

5. If said project requires a specific toolchain version, for instance 0.18.0, then it can be set with the following command:
   ```shell title=">_ Terminal"
   midenup set 0.18.0
   ```
   Note that if the toolchain is not already installed, midenup/miden will automatically install it as soon as it detects that it is required.

6. The program can be assembled with the VM, which writes `program.masb` and prints the program hash:
   ```shell title=">_ Terminal"
   miden vm compile -a program.masm
   ```

7. The program can then be executed, which prints the resulting operand stack (`[7, 0, ...]` here):
   ```shell title=">_ Terminal"
   miden vm run program.masm -i program.inputs
   ```

8. To generate a proof of the execution, which writes `program.proof` and `program.outputs`:
   ```shell title=">_ Terminal"
   miden vm prove program.masm -i program.inputs
   ```

9. Finally, the proof can be verified against the program hash printed in step 6:
   ```shell title=">_ Terminal"
   miden vm verify -p program.proof -x <PROGRAM_HASH>
   ```
