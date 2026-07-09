# Tutorial

A typical usage of midenup and miden might look like the following:

1. midenup has been downloaded and correctly configured following the instructions of the [Installation](installation.md) page or [README](https://github.com/0xMiden/midenup)
2. The latest stable toolchain can then be installed:

   ```shell title=">_ Terminal"
   midenup install stable
   ```

3. With the toolchain now installed, the installed components can be inspected with the following command:

   ```shell title=">_ Terminal"
   miden help toolchain
   ```

4. To start, create a new miden project:
   ```shell title=">_ Terminal"
   miden new miden_project && cd miden_project
   ```

5. If said project requires a specific toolchain version, for instance 0.17.0, then it can be set with the following command:
   ```shell title=">_ Terminal"
   midenup set 0.17.0
   ```
   Note that if the toolchain is not already installed, midenup/miden will automatically install it as soon as it detects that it is required.

6. With the project now generated and the required toolchain established, the `src/lib.rs` can be modified with any desired additions. Afterwards, a build can be issued:
   ```shell title=">_ Terminal"
   miden build
   ```
   Once compilation finishes, a message displaying the location of the generated Miden Package will be shown.

7. With the generated Miden Package, an account can be created and deployed with the following command:
   ```shell title=">_ Terminal"
   miden client new-account --account-type regular-account-updatable-code -p /path/to/package.masp
   ```

8. Once the account exists, a procedure exported by the package can be invoked on it with the `miden call` alias. This calls the procedure on the local account and displays both its return value and the resulting account state delta:
   ```shell title=">_ Terminal"
   miden call <ACCOUNT_ID>:<PROCEDURE> --package /path/to/package.masp
   ```
   For example, to call the `increment_count` procedure exported by a counter contract:
   ```shell title=">_ Terminal"
   miden call 0x29b8...f1:increment_count --package /path/to/counter-contract.masp
   ```
