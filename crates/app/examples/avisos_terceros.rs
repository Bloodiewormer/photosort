//! Genera el archivo THIRD-PARTY-NOTICES.txt con las licencias de dependencias y componentes externos.
//!
//! Uso:
//!   cargo run -p app --example avisos_terceros
//!   cargo run -p app --example avisos_terceros -- --check

// Writes only to THIRD-PARTY-NOTICES.txt in the repository root, never inside the photo library.
#![allow(clippy::disallowed_methods)]

use serde::Deserialize;
use std::collections::{BTreeMap, HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Deserialize)]
struct Metadata {
    packages: Vec<Package>,
    workspace_members: Vec<String>,
    resolve: Option<Resolve>,
}

#[derive(Debug, Deserialize)]
struct Package {
    id: String,
    name: String,
    version: String,
    license: Option<String>,
    manifest_path: String,
}

#[derive(Debug, Deserialize)]
struct Resolve {
    nodes: Vec<ResolveNode>,
}

#[derive(Debug, Deserialize)]
struct ResolveNode {
    id: String,
    deps: Vec<ResolveDep>,
}

#[derive(Debug, Deserialize)]
struct ResolveDep {
    pkg: String,
    dep_kinds: Vec<DepKind>,
}

#[derive(Debug, Deserialize)]
struct DepKind {
    kind: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct CrateId {
    name: String,
    version: String,
}

const LIC_MIT: &str = "Permission is hereby granted, free of charge, to any\nperson obtaining a copy of this software and associated\ndocumentation files (the \"Software\"), to deal in the\nSoftware without restriction, including without\nlimitation the rights to use, copy, modify, merge,\npublish, distribute, sublicense, and/or sell copies of\nthe Software, and to permit persons to whom the Software\nis furnished to do so, subject to the following\nconditions:\n\nThe above copyright notice and this permission notice\nshall be included in all copies or substantial portions\nof the Software.\n\nTHE SOFTWARE IS PROVIDED \"AS IS\", WITHOUT WARRANTY OF\nANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED\nTO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A\nPARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT\nSHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY\nCLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION\nOF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR\nIN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER\nDEALINGS IN THE SOFTWARE.";
const LIC_APACHE_2_0: &str = "Apache License\n                        Version 2.0, January 2004\n                     https://www.apache.org/licenses/LICENSE-2.0\n\nTERMS AND CONDITIONS FOR USE, REPRODUCTION, AND DISTRIBUTION\n\n1. Definitions.\n\n   \"License\" shall mean the terms and conditions for use, reproduction,\n   and distribution as defined by Sections 1 through 9 of this document.\n\n   \"Licensor\" shall mean the copyright owner or entity authorized by\n   the copyright owner that is granting the License.\n\n   \"Legal Entity\" shall mean the union of the acting entity and all\n   other entities that control, are controlled by, or are under common\n   control with that entity. For the purposes of this definition,\n   \"control\" means (i) the power, direct or indirect, to cause the\n   direction or management of such entity, whether by contract or\n   otherwise, or (ii) ownership of fifty percent (50%) or more of the\n   outstanding shares, or (iii) beneficial ownership of such entity.\n\n   \"You\" (or \"Your\") shall mean an individual or Legal Entity\n   exercising permissions granted by this License.\n\n   \"Source\" form shall mean the preferred form for making modifications,\n   including but not limited to software source code, documentation\n   source, and configuration files.\n\n   \"Object\" form shall mean any form resulting from mechanical\n   transformation or translation of a Source form, including but\n   not limited to compiled object code, generated documentation,\n   and conversions to other media types.\n\n   \"Work\" shall mean the work of authorship, whether in Source or\n   Object form, made available under the License, as indicated by a\n   copyright notice that is included in or attached to the work\n   (an example is provided in the Appendix below).\n\n   \"Derivative Works\" shall mean any work, whether in Source or Object\n   form, that is based on (or derived from) the Work and for which the\n   editorial revisions, annotations, elaborations, or other modifications\n   represent, as a whole, an original work of authorship. For the purposes\n   of this License, Derivative Works shall not include works that remain\n   separable from, or merely link (or bind by name) to the interfaces of,\n   the Work and Derivative Works thereof.\n\n   \"Contribution\" shall mean any work of authorship, including\n   the original version of the Work and any modifications or additions\n   to that Work or Derivative Works thereof, that is intentionally\n   submitted to Licensor for inclusion in the Work by the copyright owner\n   or by an individual or Legal Entity authorized to submit on behalf of\n   the copyright owner. For the purposes of this definition, \"submitted\"\n   means any form of electronic, verbal, or written communication sent\n   to the Licensor or its representatives, including but not limited to\n   communication on electronic mailing lists, source code control systems,\n   and issue tracking systems that are managed by, or on behalf of, the\n   Licensor for the purpose of discussing and improving the Work, but\n   excluding communication that is conspicuously marked or otherwise\n   designated in writing by the copyright owner as \"Not a Contribution.\"\n\n   \"Contributor\" shall mean Licensor and any individual or Legal Entity\n   on behalf of whom a Contribution has been received by Licensor and\n   subsequently incorporated within the Work.\n\n2. Grant of Copyright License. Subject to the terms and conditions of\n   this License, each Contributor hereby grants to You a perpetual,\n   worldwide, non-exclusive, no-charge, royalty-free, irrevocable\n   copyright license to reproduce, prepare Derivative Works of,\n   publicly display, publicly perform, sublicense, and distribute the\n   Work and such Derivative Works in Source or Object form.\n\n3. Grant of Patent License. Subject to the terms and conditions of\n   this License, each Contributor hereby grants to You a perpetual,\n   worldwide, non-exclusive, no-charge, royalty-free, irrevocable\n   (except as stated in this section) patent license to make, have made,\n   use, offer to sell, sell, import, and otherwise transfer the Work,\n   where such license applies only to those patent claims licensable\n   by such Contributor that are necessarily infringed by their\n   Contribution(s) alone or by combination of their Contribution(s)\n   with the Work to which such Contribution(s) was submitted. If You\n   institute patent litigation against any entity (including a\n   cross-claim or counterclaim in a lawsuit) alleging that the Work\n   or a Contribution incorporated within the Work constitutes direct\n   or contributory patent infringement, then any patent licenses\n   granted to You under this License for that Work shall terminate\n   as of the date such litigation is filed.\n\n4. Redistribution. You may reproduce and distribute copies of the\n   Work or Derivative Works thereof in any medium, with or without\n   modifications, and in Source or Object form, provided that You\n   meet the following conditions:\n\n   (a) You must give any other recipients of the Work or\n       Derivative Works a copy of this License; and\n\n   (b) You must cause any modified files to carry prominent notices\n       stating that You changed the files; and\n\n   (c) You must retain, in the Source form of any Derivative Works\n       that You distribute, all copyright, patent, trademark, and\n       attribution notices from the Source form of the Work,\n       excluding those notices that do not pertain to any part of\n       the Derivative Works; and\n\n   (d) If the Work includes a \"NOTICE\" text file as part of its\n       distribution, then any Derivative Works that You distribute must\n       include a readable copy of the attribution notices contained\n       within such NOTICE file, excluding those notices that do not\n       pertain to any part of the Derivative Works, in at least one\n       of the following places: within a NOTICE text file distributed\n       as part of the Derivative Works; within the Source form or\n       documentation, if provided along with the Derivative Works; or,\n       within a display generated by the Derivative Works, if and\n       wherever such third-party notices normally appear. The contents\n       of the NOTICE file are for informational purposes only and\n       do not modify the License. You may add Your own attribution\n       notices within Derivative Works that You distribute, alongside\n       or as an addendum to the NOTICE text from the Work, provided\n       that such additional attribution notices cannot be construed\n       as modifying the License.\n\n   You may add Your own copyright statement to Your modifications and\n   may provide additional or different license terms and conditions\n   for use, reproduction, or distribution of Your modifications, or\n   for any such Derivative Works as a whole, provided Your use,\n   reproduction, and distribution of the Work otherwise complies with\n   the conditions stated in this License.\n\n5. Submission of Contributions. Unless You explicitly state otherwise,\n   any Contribution intentionally submitted for inclusion in the Work\n   by You to the Licensor shall be under the terms and conditions of\n   this License, without any additional terms or conditions.\n   Notwithstanding the above, nothing herein shall supersede or modify\n   the terms of any separate license agreement you may have executed\n   with Licensor regarding such Contributions.\n\n6. Trademarks. This License does not grant permission to use the trade\n   names, trademarks, service marks, or product names of the Licensor,\n   except as required for reasonable and customary use in describing the\n   origin of the Work and reproducing the content of the NOTICE file.\n\n7. Disclaimer of Warranty. Unless required by applicable law or\n   agreed to in writing, Licensor provides the Work (and each\n   Contributor provides its Contributions) on an \"AS IS\" BASIS,\n   WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or\n   implied, including, without limitation, any warranties or conditions\n   of TITLE, NON-INFRINGEMENT, MERCHANTABILITY, or FITNESS FOR A\n   PARTICULAR PURPOSE. You are solely responsible for determining the\n   appropriateness of using or redistributing the Work and assume any\n   risks associated with Your exercise of permissions under this License.\n\n8. Limitation of Liability. In no event and under no legal theory,\n   whether in tort (including negligence), contract, or otherwise,\n   unless required by applicable law (such as deliberate and grossly\n   negligent acts) or agreed to in writing, shall any Contributor be\n   liable to You for damages, including any direct, indirect, special,\n   incidental, or consequential damages of any character arising as a\n   result of this License or out of the use or inability to use the\n   Work (including but not limited to damages for loss of goodwill,\n   work stoppage, computer failure or malfunction, or any and all\n   other commercial damages or losses), even if such Contributor\n   has been advised of the possibility of such damages.\n\n9. Accepting Warranty or Additional Liability. While redistributing\n   the Work or Derivative Works thereof, You may choose to offer,\n   and charge a fee for, acceptance of support, warranty, indemnity,\n   or other liability obligations and/or rights consistent with this\n   License. However, in accepting such obligations, You may act only\n   on Your own behalf and on Your sole responsibility, not on behalf\n   of any other Contributor, and only if You agree to indemnify,\n   defend, and hold each Contributor harmless for any liability\n   incurred by, or claims asserted against, such Contributor by reason\n   of your accepting any such warranty or additional liability.\n\nEND OF TERMS AND CONDITIONS\n\nAPPENDIX: How to apply the Apache License to your work.\n\n   To apply the Apache License to your work, attach the following\n   boilerplate notice, with the fields enclosed by brackets \"[]\"\n   replaced with your own identifying information. (Don't include\n   the brackets!)  The text should be enclosed in the appropriate\n   comment syntax for the file format. We also recommend that a\n   file or class name and description of purpose be included on the\n   same \"printed page\" as the copyright notice for easier\n   identification within third-party archives.\n\nCopyright [yyyy] [name of copyright owner]\n\nLicensed under the Apache License, Version 2.0 (the \"License\");\nyou may not use this file except in compliance with the License.\nYou may obtain a copy of the License at\n\n\thttps://www.apache.org/licenses/LICENSE-2.0\n\nUnless required by applicable law or agreed to in writing, software\ndistributed under the License is distributed on an \"AS IS\" BASIS,\nWITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.\nSee the License for the specific language governing permissions and\nlimitations under the License.";
const LIC_BSD_2_CLAUSE: &str = "BSD 2-Clause License\n\nCopyright (c) 2022-2022, the rav1e contributors\nAll rights reserved.\n\nRedistribution and use in source and binary forms, with or without\nmodification, are permitted provided that the following conditions are met:\n\n- Redistributions of source code must retain the above copyright notice, this\n  list of conditions and the following disclaimer.\n\n- Redistributions in binary form must reproduce the above copyright notice,\n  this list of conditions and the following disclaimer in the documentation\n  and/or other materials provided with the distribution.\n\nTHIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS \"AS IS\"\nAND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE\nIMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE\nDISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE\nFOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL\nDAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR\nSERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER\nCAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY,\nOR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE\nOF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.";
const LIC_BSD_3_CLAUSE: &str = "Copyright (c) 2022 Contributors to the lebe Project. All rights reserved.\n\nRedistribution and use in source and binary forms, with or without modification,\nare permitted provided that the following conditions are met:\n\n1. Redistributions of source code must retain the above copyright notice,\nthis list of conditions and the following disclaimer.\n\n2. Redistributions in binary form must reproduce the above copyright notice,\nthis list of conditions and the following disclaimer in the documentation\nand/or other materials provided with the distribution.\n\n3. Neither the name of the copyright holder nor the names of its contributors\nmay be used to endorse or promote products derived from this software without\nspecific prior written permission.\n\nTHIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS \"AS IS\"\nAND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE\nIMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE\nARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE\nLIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL\nDAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR\nSERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER\nCAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY,\nOR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE\nUSE OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.";
const LIC_ZLIB: &str = "Copyright (c) 2019 Daniel \"Lokathor\" Gee.\n\nThis software is provided 'as-is', without any express or implied warranty. In no event will the authors be held liable for any damages arising from the use of this software.\n\nPermission is granted to anyone to use this software for any purpose, including commercial applications, and to alter it and redistribute it freely, subject to the following restrictions:\n\n1. The origin of this software must not be misrepresented; you must not claim that you wrote the original software. If you use this software in a product, an acknowledgment in the product documentation would be appreciated but is not required.\n\n2. Altered source versions must be plainly marked as such, and must not be misrepresented as being the original software.\n\n3. This notice may not be removed or altered from any source distribution.";
const LIC_ISC: &str = "ISC License\n\nCopyright (c) 2019-2026, Frank Denis.\n\nPermission to use, copy, modify, and/or distribute this software for any\npurpose with or without fee is hereby granted, provided that the above\ncopyright notice and this permission notice appear in all copies.\n\nTHE SOFTWARE IS PROVIDED \"AS IS\" AND THE AUTHOR DISCLAIMS ALL WARRANTIES\nWITH REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF\nMERCHANTABILITY AND FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR\nANY SPECIAL, DIRECT, INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES\nWHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN AN\nACTION OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT OF\nOR IN CONNECTION WITH THE USE OR PERFORMANCE OF THIS SOFTWARE.";
const LIC_BSL_1_0: &str = "Boost Software License - Version 1.0 - August 17th, 2003\n\nPermission is hereby granted, free of charge, to any person or organization\nobtaining a copy of the software and accompanying documentation covered by\nthis license (the \"Software\") to use, reproduce, display, distribute,\nexecute, and transmit the Software, and to prepare derivative works of the\nSoftware, and to permit third-parties to whom the Software is furnished to\ndo so, all subject to the following:\n\nThe copyright notices in the Software and this entire statement, including\nthe above license grant, this restriction and the following disclaimer,\nmust be included in all copies of the Software, in whole or in part, and\nall derivative works of the Software, unless such copies or derivative\nworks are solely in the form of machine-executable object code generated by\na source language processor.\n\nTHE SOFTWARE IS PROVIDED \"AS IS\", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR\nIMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,\nFITNESS FOR A PARTICULAR PURPOSE, TITLE AND NON-INFRINGEMENT. IN NO EVENT\nSHALL THE COPYRIGHT HOLDERS OR ANYONE DISTRIBUTING THE SOFTWARE BE LIABLE\nFOR ANY DAMAGES OR OTHER LIABILITY, WHETHER IN CONTRACT, TORT OR OTHERWISE,\nARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER\nDEALINGS IN THE SOFTWARE.";
const LIC_UNICODE_3_0: &str = "UNICODE LICENSE V3\n\nCOPYRIGHT AND PERMISSION NOTICE\n\nCopyright © 1991-2023 Unicode, Inc.\n\nNOTICE TO USER: Carefully read the following legal agreement. BY\nDOWNLOADING, INSTALLING, COPYING OR OTHERWISE USING DATA FILES, AND/OR\nSOFTWARE, YOU UNEQUIVOCALLY ACCEPT, AND AGREE TO BE BOUND BY, ALL OF THE\nTERMS AND CONDITIONS OF THIS AGREEMENT. IF YOU DO NOT AGREE, DO NOT\nDOWNLOAD, INSTALL, COPY, DISTRIBUTE OR USE THE DATA FILES OR SOFTWARE.\n\nPermission is hereby granted, free of charge, to any person obtaining a\ncopy of data files and any associated documentation (the \"Data Files\") or\nsoftware and any associated documentation (the \"Software\") to deal in the\nData Files or Software without restriction, including without limitation\nthe rights to use, copy, modify, merge, publish, distribute, and/or sell\ncopies of the Data Files or Software, and to permit persons to whom the\nData Files or Software are furnished to do so, provided that either (a)\nthis copyright and permission notice appear with all copies of the Data\nFiles or Software, or (b) this copyright and permission notice appear in\nassociated Documentation.\n\nTHE DATA FILES AND SOFTWARE ARE PROVIDED \"AS IS\", WITHOUT WARRANTY OF ANY\nKIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF\nMERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT OF\nTHIRD PARTY RIGHTS.\n\nIN NO EVENT SHALL THE COPYRIGHT HOLDER OR HOLDERS INCLUDED IN THIS NOTICE\nBE LIABLE FOR ANY CLAIM, OR ANY SPECIAL INDIRECT OR CONSEQUENTIAL DAMAGES,\nOR ANY DAMAGES WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS,\nWHETHER IN AN ACTION OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION,\nARISING OUT OF OR IN CONNECTION WITH THE USE OR PERFORMANCE OF THE DATA\nFILES OR SOFTWARE.\n\nExcept as contained in this notice, the name of a copyright holder shall\nnot be used in advertising or otherwise to promote the sale, use or other\ndealings in these Data Files or Software without prior written\nauthorization of the copyright holder.";
const LIC_CC0_1_0: &str = "Creative Commons Legal Code\n\nCC0 1.0 Universal\n\n    CREATIVE COMMONS CORPORATION IS NOT A LAW FIRM AND DOES NOT PROVIDE\n    LEGAL SERVICES. DISTRIBUTION OF THIS DOCUMENT DOES NOT CREATE AN\n    ATTORNEY-CLIENT RELATIONSHIP. CREATIVE COMMONS PROVIDES THIS\n    INFORMATION ON AN \"AS-IS\" BASIS. CREATIVE COMMONS MAKES NO WARRANTIES\n    REGARDING THE USE OF THIS DOCUMENT OR THE INFORMATION OR WORKS\n    PROVIDED HEREUNDER, AND DISCLAIMS LIABILITY FOR DAMAGES RESULTING FROM\n    THE USE OF THIS DOCUMENT OR THE INFORMATION OR WORKS PROVIDED\n    HEREUNDER.\n\nStatement of Purpose\n\nThe laws of most jurisdictions throughout the world automatically confer\nexclusive Copyright and Related Rights (defined below) upon the creator\nand subsequent owner(s) (each and all, an \"owner\") of an original work of\nauthorship and/or a database (each, a \"Work\").\n\nCertain owners wish to permanently relinquish those rights to a Work for\nthe purpose of contributing to a commons of creative, cultural and\nscientific works (\"Commons\") that the public can reliably and without fear\nof later claims of infringement build upon, modify, incorporate in other\nworks, reuse and redistribute as freely as possible in any form whatsoever\nand for any purposes, including without limitation commercial purposes.\nThese owners may contribute to the Commons to promote the ideal of a free\nculture and the further production of creative, cultural and scientific\nworks, or to gain reputation or greater distribution for their Work in\npart through the use and efforts of others.\n\nFor these and/or other purposes and motivations, and without any\nexpectation of additional consideration or compensation, the person\nassociating CC0 with a Work (the \"Affirmer\"), to the extent that he or she\nis an owner of Copyright and Related Rights in the Work, voluntarily\nelects to apply CC0 to the Work and publicly distribute the Work under its\nterms, with knowledge of his or her Copyright and Related Rights in the\nWork and the meaning and intended legal effect of CC0 on those rights.\n\n1. Copyright and Related Rights. A Work made available under CC0 may be\nprotected by copyright and related or neighboring rights (\"Copyright and\nRelated Rights\"). Copyright and Related Rights include, but are not\nlimited to, the following:\n\n  i. the right to reproduce, adapt, distribute, perform, display,\n     communicate, and translate a Work;\n ii. moral rights retained by the original author(s) and/or performer(s);\niii. publicity and privacy rights pertaining to a person's image or\n     likeness depicted in a Work;\n iv. rights protecting against unfair competition in regards to a Work,\n     subject to the limitations in paragraph 4(a), below;\n  v. rights protecting the extraction, dissemination, use and reuse of data\n     in a Work;\n vi. database rights (such as those arising under Directive 96/9/EC of the\n     European Parliament and of the Council of 11 March 1996 on the legal\n     protection of databases, and under any national implementation\n     thereof, including any amended or successor version of such\n     directive); and\nvii. other similar, equivalent or corresponding rights throughout the\n     world based on applicable law or treaty, and any national\n     implementations thereof.\n\n2. Waiver. To the greatest extent permitted by, but not in contravention\nof, applicable law, Affirmer hereby overtly, fully, permanently,\nirrevocably and unconditionally waives, abandons, and surrenders all of\nAffirmer's Copyright and Related Rights and associated claims and causes\nof action, whether now known or unknown (including existing as well as\nfuture claims and causes of action), in the Work (i) in all territories\nworldwide, (ii) for the maximum duration provided by applicable law or\ntreaty (including future time extensions), (iii) in any current or future\nmedium and for any number of copies, and (iv) for any purpose whatsoever,\nincluding without limitation commercial, advertising or promotional\npurposes (the \"Waiver\"). Affirmer makes the Waiver for the benefit of each\nmember of the public at large and to the detriment of Affirmer's heirs and\nsuccessors, fully intending that such Waiver shall not be subject to\nrevocation, rescission, cancellation, termination, or any other legal or\nequitable action to disrupt the quiet enjoyment of the Work by the public\nas contemplated by Affirmer's express Statement of Purpose.\n\n3. Public License Fallback. Should any part of the Waiver for any reason\nbe judged legally invalid or ineffective under applicable law, then the\nWaiver shall be preserved to the maximum extent permitted taking into\naccount Affirmer's express Statement of Purpose. In addition, to the\nextent the Waiver is so judged Affirmer hereby grants to each affected\nperson a royalty-free, non transferable, non sublicensable, non exclusive,\nirrevocable and unconditional license to exercise Affirmer's Copyright and\nRelated Rights in the Work (i) in all territories worldwide, (ii) for the\nmaximum duration provided by applicable law or treaty (including future\ntime extensions), (iii) in any current or future medium and for any number\nof copies, and (iv) for any purpose whatsoever, including without\nlimitation commercial, advertising or promotional purposes (the\n\"License\"). The License shall be deemed effective as of the date CC0 was\napplied by Affirmer to the Work. Should any part of the License for any\nreason be judged legally invalid or ineffective under applicable law, such\npartial invalidity or ineffectiveness shall not invalidate the remainder\nof the License, and in such case Affirmer hereby affirms that he or she\nwill not (i) exercise any of his or her remaining Copyright and Related\nRights in the Work or (ii) assert any associated claims and causes of\naction with respect to the Work, in either case contrary to Affirmer's\nexpress Statement of Purpose.\n\n4. Limitations and Disclaimers.\n\n a. No trademark or patent rights held by Affirmer are waived, abandoned,\n    surrendered, licensed or otherwise affected by this document.\n b. Affirmer offers the Work as-is and makes no representations or\n    warranties of any kind concerning the Work, express, implied,\n    statutory or otherwise, including without limitation warranties of\n    title, merchantability, fitness for a particular purpose, non\n    infringement, or the absence of latent or other defects, accuracy, or\n    the present or absence of errors, whether or not discoverable, all to\n    the greatest extent permissible under applicable law.\n c. Affirmer disclaims responsibility for clearing rights of other persons\n    that may apply to the Work or any use thereof, including without\n    limitation any person's Copyright and Related Rights in the Work.\n    Further, Affirmer disclaims responsibility for obtaining any necessary\n    consents, permissions or other rights required for any use of the\n    Work.\n d. Affirmer understands and acknowledges that Creative Commons is not a\n    party to this document and has no duty or obligation with respect to\n    this CC0 or use of the Work.";
const LIC_ZERO_BSD: &str = "Copyright (C) Jonas Schievink <jonasschievink@gmail.com>\n\nPermission to use, copy, modify, and/or distribute this software for\nany purpose with or without fee is hereby granted.\n\nTHE SOFTWARE IS PROVIDED \"AS IS\" AND THE AUTHOR DISCLAIMS ALL WARRANTIES\nWITH REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF\nMERCHANTABILITY AND FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR\nANY SPECIAL, DIRECT, INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES\nWHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN\nAN ACTION OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT\nOF OR IN CONNECTION WITH THE USE OR PERFORMANCE OF THIS SOFTWARE.";
const LIC_UNLICENSE: &str = "This is free and unencumbered software released into the public domain.\n\nAnyone is free to copy, modify, publish, use, compile, sell, or\ndistribute this software, either in source code form or as a compiled\nbinary, for any purpose, commercial or non-commercial, and by any\nmeans.\n\nIn jurisdictions that recognize copyright laws, the author or authors\nof this software dedicate any and all copyright interest in the\nsoftware to the public domain. We make this dedication for the benefit\nof the public at large and to the detriment of our heirs and\nsuccessors. We intend this dedication to be an overt act of\nrelinquishment in perpetuity of all present and future rights to this\nsoftware under copyright law.\n\nTHE SOFTWARE IS PROVIDED \"AS IS\", WITHOUT WARRANTY OF ANY KIND,\nEXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF\nMERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT.\nIN NO EVENT SHALL THE AUTHORS BE LIABLE FOR ANY CLAIM, DAMAGES OR\nOTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE,\nARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR\nOTHER DEALINGS IN THE SOFTWARE.\n\nFor more information, please refer to <http://unlicense.org/>";
const LIC_MIT_0: &str = "Permission is hereby granted, free of charge, to any person obtaining a\ncopy of this software and associated documentation files (the\n\"Software\"), to deal in the Software without restriction, including\nwithout limitation the rights to use, copy, modify, merge, publish,\ndistribute, sublicense, and/or sell copies of the Software, and to\npermit persons to whom the Software is furnished to do so.\n\nTHE SOFTWARE IS PROVIDED \"AS IS\", WITHOUT WARRANTY OF ANY KIND, EXPRESS\nOR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF\nMERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT.\nIN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY\nCLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT,\nTORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE\nSOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.";
const LIC_OFL_1_1: &str = "This Font Software is licensed under the SIL Open Font License,\nVersion 1.1.\n\nThis license is copied below, and is also available with a FAQ at:\nhttp://scripts.sil.org/OFL\n\n-----------------------------------------------------------\nSIL OPEN FONT LICENSE Version 1.1 - 26 February 2007\n-----------------------------------------------------------\n\nPREAMBLE\nThe goals of the Open Font License (OFL) are to stimulate worldwide\ndevelopment of collaborative font projects, to support the font\ncreation efforts of academic and linguistic communities, and to\nprovide a free and open framework in which fonts may be shared and\nimproved in partnership with others.\n\nThe OFL allows the licensed fonts to be used, studied, modified and\nredistributed freely as long as they are not sold by themselves. The\nfonts, including any derivative works, can be bundled, embedded,\nredistributed and/or sold with any software provided that any reserved\nnames are not used by derivative works. The fonts and derivatives,\nhowever, cannot be released under any other type of license. The\nrequirement for fonts to remain under this license does not apply to\nany document created using the fonts or their derivatives.\n\nDEFINITIONS\n\"Font Software\" refers to the set of files released by the Copyright\nHolder(s) under this license and clearly marked as such. This may\ninclude source files, build scripts and documentation.\n\n\"Reserved Font Name\" refers to any names specified as such after the\ncopyright statement(s).\n\n\"Original Version\" refers to the collection of Font Software\ncomponents as distributed by the Copyright Holder(s).\n\n\"Modified Version\" refers to any derivative made by adding to,\ndeleting, or substituting -- in part or in whole -- any of the\ncomponents of the Original Version, by changing formats or by porting\nthe Font Software to a new environment.\n\n\"Author\" refers to any designer, engineer, programmer, technical\nwriter or other person who contributed to the Font Software.\n\nPERMISSION & CONDITIONS\nPermission is hereby granted, free of charge, to any person obtaining\na copy of the Font Software, to use, study, copy, merge, embed,\nmodify, redistribute, and sell modified and unmodified copies of the\nFont Software, subject to the following conditions:\n\n1) Neither the Font Software nor any of its individual components, in\nOriginal or Modified Versions, may be sold by itself.\n\n2) Original or Modified Versions of the Font Software may be bundled,\nredistributed and/or sold with any software, provided that each copy\ncontains the above copyright notice and this license. These can be\nincluded either as stand-alone text files, human-readable headers or\nin the appropriate machine-readable metadata fields within text or\nbinary files as long as those fields can be easily viewed by the user.\n\n3) No Modified Version of the Font Software may use the Reserved Font\nName(s) unless explicit written permission is granted by the\ncorresponding Copyright Holder. This restriction only applies to the\nprimary font name as presented to the users.\n\n4) The name(s) of the Copyright Holder(s) or the Author(s) of the Font\nSoftware shall not be used to promote, endorse or advertise any\nModified Version, except to acknowledge the contribution(s) of the\nCopyright Holder(s) and the Author(s) or with their explicit written\npermission.\n\n5) The Font Software, modified or unmodified, in part or in whole,\nmust be distributed entirely under this license, and must not be\ndistributed under any other license. The requirement for fonts to\nremain under this license does not apply to any document created using\nthe Font Software.\n\nTERMINATION\nThis license becomes null and void if any of the above conditions are\nnot met.\n\nDISCLAIMER\nTHE FONT SOFTWARE IS PROVIDED \"AS IS\", WITHOUT WARRANTY OF ANY KIND,\nEXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO ANY WARRANTIES OF\nMERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT\nOF COPYRIGHT, PATENT, TRADEMARK, OR OTHER RIGHT. IN NO EVENT SHALL THE\nCOPYRIGHT HOLDER BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY,\nINCLUDING ANY GENERAL, SPECIAL, INDIRECT, INCIDENTAL, OR CONSEQUENTIAL\nDAMAGES, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING\nFROM, OUT OF THE USE OR INABILITY TO USE THE FONT SOFTWARE OR FROM\nOTHER DEALINGS IN THE FONT SOFTWARE.";
const LIC_CDLA_PERMISSIVE_2_0: &str = "# Community Data License Agreement - Permissive - Version 2.0\n\nThis is the Community Data License Agreement - Permissive, Version\n2.0 (the \"agreement\"). Data Provider(s) and Data Recipient(s) agree\nas follows:\n\n## 1. Provision of the Data\n\n1.1. A Data Recipient may use, modify, and share the Data made\navailable by Data Provider(s) under this agreement if that Data\nRecipient follows the terms of this agreement.\n\n1.2. This agreement does not impose any restriction on a Data\nRecipient's use, modification, or sharing of any portions of the\nData that are in the public domain or that may be used, modified,\nor shared under any other legal exception or limitation.\n\n## 2. Conditions for Sharing Data\n\n2.1. A Data Recipient may share Data, with or without modifications, so\nlong as the Data Recipient makes available the text of this agreement\nwith the shared Data.\n\n## 3. No Restrictions on Results\n\n3.1. This agreement does not impose any restriction or obligations\nwith respect to the use, modification, or sharing of Results.\n\n## 4. No Warranty; Limitation of Liability\n\n4.1. All Data Recipients receive the Data subject to the following\nterms:\n\nTHE DATA IS PROVIDED ON AN \"AS IS\" BASIS, WITHOUT REPRESENTATIONS,\nWARRANTIES OR CONDITIONS OF ANY KIND, EITHER EXPRESS OR IMPLIED\nINCLUDING, WITHOUT LIMITATION, ANY WARRANTIES OR CONDITIONS OF TITLE,\nNON-INFRINGEMENT, MERCHANTABILITY OR FITNESS FOR A PARTICULAR PURPOSE.\n\nNO DATA PROVIDER SHALL HAVE ANY LIABILITY FOR ANY DIRECT, INDIRECT,\nINCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING\nWITHOUT LIMITATION LOST PROFITS), HOWEVER CAUSED AND ON ANY THEORY OF\nLIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING\nNEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE DATA OR RESULTS,\nEVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGES.\n\n## 5. Definitions\n\n5.1. \"Data\" means the material received by a Data Recipient under\nthis agreement.\n\n5.2. \"Data Provider\" means any person who is the source of Data\nprovided under this agreement and in reliance on a Data Recipient's\nagreement to its terms.\n\n5.3. \"Data Recipient\" means any person who receives Data directly\nor indirectly from a Data Provider and agrees to the terms of this\nagreement.\n\n5.4. \"Results\" means any outcome obtained by computational analysis\nof Data, including for example machine learning models and models'\ninsights.";
const LIC_UBUNTU_FONT_1_0: &str = "-------------------------------\nUBUNTU FONT LICENCE Version 1.0\n-------------------------------\n\nPREAMBLE\nThis licence allows the licensed fonts to be used, studied, modified and\nredistributed freely. The fonts, including any derivative works, can be\nbundled, embedded, and redistributed provided the terms of this licence\nare met. The fonts and derivatives, however, cannot be released under\nany other licence. The requirement for fonts to remain under this\nlicence does not require any document created using the fonts or their\nderivatives to be published under this licence, as long as the primary\npurpose of the document is not to be a vehicle for the distribution of\nthe fonts.\n\nDEFINITIONS\n\"Font Software\" refers to the set of files released by the Copyright\nHolder(s) under this licence and clearly marked as such. This may\ninclude source files, build scripts and documentation.\n\n\"Original Version\" refers to the collection of Font Software components\nas received under this licence.\n\n\"Modified Version\" refers to any derivative made by adding to, deleting,\nor substituting -- in part or in whole -- any of the components of the\nOriginal Version, by changing formats or by porting the Font Software to\na new environment.\n\n\"Copyright Holder(s)\" refers to all individuals and companies who have a\ncopyright ownership of the Font Software.\n\n\"Substantially Changed\" refers to Modified Versions which can be easily\nidentified as dissimilar to the Font Software by users of the Font\nSoftware comparing the Original Version with the Modified Version.\n\nTo \"Propagate\" a work means to do anything with it that, without\npermission, would make you directly or secondarily liable for\ninfringement under applicable copyright law, except executing it on a\ncomputer or modifying a private copy. Propagation includes copying,\ndistribution (with or without modification and with or without charging\na redistribution fee), making available to the public, and in some\ncountries other activities as well.\n\nPERMISSION & CONDITIONS\nThis licence does not grant any rights under trademark law and all such\nrights are reserved.\n\nPermission is hereby granted, free of charge, to any person obtaining a\ncopy of the Font Software, to propagate the Font Software, subject to\nthe below conditions:\n\n1) Each copy of the Font Software must contain the above copyright\nnotice and this licence. These can be included either as stand-alone\ntext files, human-readable headers or in the appropriate machine-\nreadable metadata fields within text or binary files as long as those\nfields can be easily viewed by the user.\n\n2) The font name complies with the following:\n(a) The Original Version must retain its name, unmodified.\n(b) Modified Versions which are Substantially Changed must be renamed to\navoid use of the name of the Original Version or similar names entirely.\n(c) Modified Versions which are not Substantially Changed must be\nrenamed to both (i) retain the name of the Original Version and (ii) add\nadditional naming elements to distinguish the Modified Version from the\nOriginal Version. The name of such Modified Versions must be the name of\nthe Original Version, with \"derivative X\" where X represents the name of\nthe new work, appended to that name.\n\n3) The name(s) of the Copyright Holder(s) and any contributor to the\nFont Software shall not be used to promote, endorse or advertise any\nModified Version, except (i) as required by this licence, (ii) to\nacknowledge the contribution(s) of the Copyright Holder(s) or (iii) with\ntheir explicit written permission.\n\n4) The Font Software, modified or unmodified, in part or in whole, must\nbe distributed entirely under this licence, and must not be distributed\nunder any other licence. The requirement for fonts to remain under this\nlicence does not affect any document created using the Font Software,\nexcept any version of the Font Software extracted from a document\ncreated using the Font Software may only be distributed under this\nlicence.\n\nTERMINATION\nThis licence becomes null and void if any of the above conditions are\nnot met.\n\nDISCLAIMER\nTHE FONT SOFTWARE IS PROVIDED \"AS IS\", WITHOUT WARRANTY OF ANY KIND,\nEXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO ANY WARRANTIES OF\nMERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT OF\nCOPYRIGHT, PATENT, TRADEMARK, OR OTHER RIGHT. IN NO EVENT SHALL THE\nCOPYRIGHT HOLDER BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY,\nINCLUDING ANY GENERAL, SPECIAL, INDIRECT, INCIDENTAL, OR CONSEQUENTIAL\nDAMAGES, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING\nFROM, OUT OF THE USE OR INABILITY TO USE THE FONT SOFTWARE OR FROM OTHER\nDEALINGS IN THE FONT SOFTWARE.";

fn fallback_spdx_license(expr: &str) -> String {
    let clean = expr.trim();
    if clean.contains("AND") {
        let parts: Vec<&str> = clean.split("AND").map(|s| s.trim()).collect();
        let texts: Vec<String> = parts.iter().map(|p| fallback_single_spdx(p)).collect();
        return texts.join("\n\n================================================================================\n\n");
    }
    fallback_single_spdx(clean)
}

fn fallback_single_spdx(part: &str) -> String {
    let s = part.trim().trim_matches(|c| c == '(' || c == ')').trim();
    if s.contains("MIT") {
        return LIC_MIT.to_string();
    }
    if s.contains("Apache-2.0") {
        return LIC_APACHE_2_0.to_string();
    }
    if s == "BSD-2-Clause" {
        return LIC_BSD_2_CLAUSE.to_string();
    }
    if s == "BSD-3-Clause" {
        return LIC_BSD_3_CLAUSE.to_string();
    }
    if s == "Zlib" {
        return LIC_ZLIB.to_string();
    }
    if s == "ISC" {
        return LIC_ISC.to_string();
    }
    if s == "BSL-1.0" {
        return LIC_BSL_1_0.to_string();
    }
    if s.contains("Unicode") {
        return LIC_UNICODE_3_0.to_string();
    }
    if s.contains("CC0") {
        return LIC_CC0_1_0.to_string();
    }
    if s.contains("0BSD") {
        return LIC_ZERO_BSD.to_string();
    }
    if s.contains("Unlicense") {
        return LIC_UNLICENSE.to_string();
    }
    if s.contains("MIT-0") {
        return LIC_MIT_0.to_string();
    }
    if s.contains("OFL") {
        return LIC_OFL_1_1.to_string();
    }
    if s.contains("CDLA") {
        return LIC_CDLA_PERMISSIVE_2_0.to_string();
    }
    if s.contains("Ubuntu") {
        return LIC_UBUNTU_FONT_1_0.to_string();
    }
    panic!("Licencia SPDX desconocida sin texto de reserva: {}", s);
}

fn read_crate_license_text(p: &Package) -> String {
    let manifest_path = Path::new(&p.manifest_path);
    let crate_dir = manifest_path.parent().expect("manifest parent dir");

    let mut license_files = Vec::new();
    if let Ok(entries) = fs::read_dir(crate_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let is_match = path.is_file()
                && path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|name| {
                        let upper = name.to_ascii_uppercase();
                        upper.starts_with("LICENSE")
                            || upper.starts_with("COPYING")
                            || upper.starts_with("NOTICE")
                    });
            if is_match {
                license_files.push(path);
            }
        }
    }

    license_files.sort_by(|a, b| a.file_name().cmp(&b.file_name()));

    let mut combined = Vec::new();
    for f in license_files {
        if let Ok(raw) = fs::read_to_string(&f) {
            let normalized = raw.replace("\r\n", "\n").trim().to_string();
            if !normalized.is_empty() {
                combined.push(normalized);
            }
        }
    }

    if !combined.is_empty() {
        combined.join("\n\n").trim().to_string()
    } else if let Some(expr) = &p.license {
        fallback_spdx_license(expr)
            .replace("\r\n", "\n")
            .trim()
            .to_string()
    } else {
        panic!(
            "La crate {} {} no tiene archivos de licencia ni campo license",
            p.name, p.version
        );
    }
}

fn generate_notices() -> (String, usize) {
    let output = Command::new("cargo")
        .args([
            "metadata",
            "--format-version",
            "1",
            "--locked",
            "--filter-platform",
            "x86_64-pc-windows-msvc",
        ])
        .output()
        .expect("fallo al ejecutar cargo metadata");

    if !output.status.success() {
        panic!(
            "cargo metadata devolvio error: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let meta: Metadata =
        serde_json::from_slice(&output.stdout).expect("fallo al deserializar cargo metadata");

    let packages_by_id: std::collections::HashMap<_, _> =
        meta.packages.iter().map(|p| (p.id.clone(), p)).collect();

    let nodes_by_id: std::collections::HashMap<_, _> = meta
        .resolve
        .as_ref()
        .expect("resolve no disponible en metadata")
        .nodes
        .iter()
        .map(|n| (n.id.clone(), n))
        .collect();

    let app_pkg = meta
        .packages
        .iter()
        .find(|p| p.name == "app")
        .expect("paquete app no encontrado en workspace");

    let workspace_set: HashSet<_> = meta.workspace_members.iter().cloned().collect();

    let mut visited = HashSet::new();
    let mut queue = VecDeque::new();
    visited.insert(app_pkg.id.clone());
    queue.push_back(app_pkg.id.clone());

    while let Some(curr_id) = queue.pop_front() {
        if let Some(node) = nodes_by_id.get(&curr_id) {
            for dep in &node.deps {
                let is_runtime = dep.dep_kinds.iter().any(|k| k.kind.is_none());
                if is_runtime && visited.insert(dep.pkg.clone()) {
                    queue.push_back(dep.pkg.clone());
                }
            }
        }
    }

    let mut third_party_pkgs: Vec<&Package> = visited
        .iter()
        .filter(|id| !workspace_set.contains(*id))
        .map(|id| *packages_by_id.get(id).expect("paquete en id"))
        .collect();

    third_party_pkgs.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.version.cmp(&b.version)));
    let total_crates = third_party_pkgs.len();

    let mut text_to_crates: BTreeMap<String, Vec<CrateId>> = BTreeMap::new();
    for p in third_party_pkgs {
        let text = read_crate_license_text(p);
        let cid = CrateId {
            name: p.name.clone(),
            version: p.version.clone(),
        };
        text_to_crates.entry(text).or_default().push(cid);
    }

    let mut groups: Vec<(Vec<CrateId>, String)> = Vec::new();
    for (text, mut crates) in text_to_crates {
        crates.sort();
        groups.push((crates, text));
    }

    groups.sort_by(|a, b| a.0[0].cmp(&b.0[0]));

    let mut out = String::new();
    out.push_str(
        "================================================================================\n",
    );
    out.push_str("AVISOS DE TERCEROS Y LICENCIAS DE PHOTOSORT\n");
    out.push_str(
        "================================================================================\n\n",
    );
    out.push_str("Este archivo contiene los avisos de derechos de autor y licencias de los\n");
    out.push_str(
        "componentes de terceros distribuidos con la aplicacion o utilizados por ella.\n\n",
    );
    out.push_str("Contenido:\n");
    out.push_str("  1. Dependencias de ejecucion de Rust (crates compiladas en photosort.exe)\n");
    out.push_str("  2. Componentes nativos y bibliotecas externas (ONNX Runtime, DirectML)\n");
    out.push_str("  3. Modelos de inteligencia artificial (evaluacion y catalogacion local)\n\n");

    out.push_str(
        "================================================================================\n",
    );
    out.push_str(&format!("1. CRATES DE RUST (DEPENDENCIAS DE EJECUCION PARA x86_64-pc-windows-msvc)\nTotal de crates incluidas: {}\n", total_crates));
    out.push_str(
        "================================================================================\n\n",
    );

    for (crates, text) in groups {
        out.push_str(
            "--------------------------------------------------------------------------------\n",
        );
        if crates.len() == 1 {
            out.push_str(&format!(
                "Crate: {} {}\n",
                crates[0].name, crates[0].version
            ));
        } else {
            out.push_str(&format!("Crates ({}):\n", crates.len()));
            for c in crates {
                out.push_str(&format!("  - {} {}\n", c.name, c.version));
            }
        }
        out.push_str(
            "--------------------------------------------------------------------------------\n\n",
        );
        out.push_str(&text);
        out.push_str("\n\n");
    }

    out.push_str(
        "================================================================================\n",
    );
    out.push_str("2. COMPONENTES NATIVOS Y BIBLIOTECAS EXTERNAS\n");
    out.push_str(
        "================================================================================\n\n",
    );

    out.push_str(
        "--------------------------------------------------------------------------------\n",
    );
    out.push_str("ONNX Runtime 1.31 (rel-1.31.0)\n");
    out.push_str(
        "--------------------------------------------------------------------------------\n",
    );
    out.push_str("Licencia: MIT\n");
    out.push_str("Copyright (c) Microsoft Corporation. All rights reserved.\n\n");
    out.push_str("Nota de componentes de terceros de ONNX Runtime:\n");
    out.push_str("La biblioteca estatica incluye componentes de terceros (entre ellos Eigen con\n");
    out.push_str(
        "licencia MPL-2.0, cuyo codigo fuente sin modificar esta disponible en upstream).\n",
    );
    out.push_str("El listado completo de licencias y avisos de terceros de ONNX Runtime 1.31\n");
    out.push_str("se encuentra en:\n");
    out.push_str(
        "  https://github.com/microsoft/onnxruntime/blob/rel-1.31.0/ThirdPartyNotices.txt\n\n",
    );
    out.push_str(LIC_MIT);
    out.push_str("\n\n");

    out.push_str(
        "--------------------------------------------------------------------------------\n",
    );
    out.push_str("DirectML 1.15.4 (DirectML.dll)\n");
    out.push_str(
        "--------------------------------------------------------------------------------\n",
    );
    out.push_str(
        "Licencia: Microsoft Software License Terms, DirectML (propietaria de Microsoft)\n",
    );
    out.push_str("Terminos de la licencia:\n");
    out.push_str("  https://www.nuget.org/packages/Microsoft.AI.DirectML/1.15.4/License\n\n");
    out.push_str("Nota de redistribucion:\n");
    out.push_str(
        "DirectML.dll se distribuye exclusivamente incorporada junto al ejecutable dentro\n",
    );
    out.push_str(
        "de la aplicacion photosort para su uso en Windows o Xbox. No se redistribuye de\n",
    );
    out.push_str(
        "forma aislada, no se ofrece como descarga suelta, ni se alteran o suprimen los\n",
    );
    out.push_str("avisos legales y de derechos de autor de Microsoft.\n\n");

    out.push_str(
        "================================================================================\n",
    );
    out.push_str("3. MODELOS DE INTELIGENCIA ARTIFICIAL\n");
    out.push_str(
        "(No van dentro del ejecutable .exe; se almacenan en la carpeta de modelos del usuario)\n",
    );
    out.push_str(
        "================================================================================\n\n",
    );

    out.push_str(
        "--------------------------------------------------------------------------------\n",
    );
    out.push_str("SigLIP 2 (google/siglip2-base-patch16-256)\n");
    out.push_str(
        "--------------------------------------------------------------------------------\n",
    );
    out.push_str("Archivos:\n");
    out.push_str("  - siglip2/vision_model_fp16.onnx\n");
    out.push_str("  - siglip2/text_model_int8.onnx\n");
    out.push_str("  - siglip2/tokenizer.json\n");
    out.push_str("  - siglip2/config.json\n");
    out.push_str("  - siglip2/preprocessor_config.json\n");
    out.push_str("  - siglip2/tokenizer_config.json\n");
    out.push_str("Origen:\n");
    out.push_str("  onnx-community/siglip2-base-patch16-256-ONNX rev d1114256522a37ffa257a0a58017348ab0058db2\n");
    out.push_str("  (modelo base de Google: google/siglip2-base-patch16-256)\n");
    out.push_str("Licencia: Apache-2.0\n");
    out.push_str(
        "Exigencias: Conservar texto de la licencia Apache-2.0 y avisos NOTICE si existieran.\n\n",
    );

    out.push_str(
        "--------------------------------------------------------------------------------\n",
    );
    out.push_str("YuNet (Deteccion de rostros)\n");
    out.push_str(
        "--------------------------------------------------------------------------------\n",
    );
    out.push_str("Archivo:\n");
    out.push_str("  - yunet/face_detection_yunet_2023mar.onnx\n");
    out.push_str("Origen:\n");
    out.push_str(
        "  opencv/face_detection_yunet rev 3cc26e7f1014a5ee5d74a42acee58bafc9d0a310 (OpenCV Zoo)\n",
    );
    out.push_str("Licencia: MIT\n");
    out.push_str(
        "Exigencias: Conservar aviso de derechos de autor y texto de la licencia MIT.\n\n",
    );

    out.push_str(
        "--------------------------------------------------------------------------------\n",
    );
    out.push_str("SFace (Reconocimiento facial)\n");
    out.push_str(
        "--------------------------------------------------------------------------------\n",
    );
    out.push_str("Archivo:\n");
    out.push_str("  - sface/face_recognition_sface_2021dec.onnx\n");
    out.push_str("Origen:\n");
    out.push_str("  opencv/face_recognition_sface rev 3d7082438a6e4551e840c9b2bb60b71e8da4b524 (OpenCV Zoo)\n");
    out.push_str("Licencia: Apache-2.0\n");
    out.push_str(
        "Exigencias: Conservar texto de la licencia Apache-2.0 y avisos NOTICE si existieran.\n\n",
    );

    out.push_str(
        "--------------------------------------------------------------------------------\n",
    );
    out.push_str("PP-OCRv6 y reconocedor coreano PP-OCRv5 (PaddleOCR)\n");
    out.push_str(
        "--------------------------------------------------------------------------------\n",
    );
    out.push_str("Archivos:\n");
    out.push_str("  - pp-ocrv6_small_det.onnx (v0.7.0 4e350472b058ef78a640634bc9095fc3836c3e34)\n");
    out.push_str("  - pp-ocrv6_small_rec.onnx (v0.7.0)\n");
    out.push_str("  - ppocrv6_dict.txt (v0.7.0)\n");
    out.push_str(
        "  - korean_pp-ocrv5_mobile_rec.onnx (v0.3.0 7a0ea9cbe5de7d9e4c8415b032f4854fb688245e)\n",
    );
    out.push_str("  - ppocrv5_korean_dict.txt (v0.3.0)\n");
    out.push_str("Origen:\n");
    out.push_str("  PaddlePaddle / PaddleOCR (https://github.com/PaddlePaddle/PaddleOCR)\n");
    out.push_str("  Publicaciones upstream: GreatV/oar-ocr y HuggingFace PaddlePaddle/korean_PP-OCRv5_mobile_rec\n");
    out.push_str("Licencia: Apache-2.0\n");
    out.push_str(
        "Exigencias: Conservar texto de la licencia Apache-2.0 y avisos NOTICE si existieran.\n\n",
    );

    (out, total_crates)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let is_check = args.iter().any(|a| a == "--check");

    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("no se pudo determinar la raiz del repositorio");

    let target_path = if let Some(p) = args.iter().find(|a| !a.starts_with("--")) {
        PathBuf::from(p)
    } else {
        repo_root.join("THIRD-PARTY-NOTICES.txt")
    };

    let (content, total_crates) = generate_notices();

    if is_check {
        match fs::read_to_string(&target_path) {
            Ok(existing) => {
                let existing_norm = existing.replace("\r\n", "\n");
                let content_norm = content.replace("\r\n", "\n");
                if existing_norm == content_norm {
                    println!(
                        "OK: {} esta actualizado ({} crates).",
                        target_path.display(),
                        total_crates
                    );
                } else {
                    eprintln!(
                        "ERROR: {} difiere del contenido generado.",
                        target_path.display()
                    );
                    eprintln!(
                        "Ejecuta 'cargo run -p app --example avisos_terceros' para regenerarlo."
                    );
                    std::process::exit(1);
                }
            }
            Err(e) => {
                eprintln!("ERROR: no se pudo leer {}: {}", target_path.display(), e);
                eprintln!("Ejecuta 'cargo run -p app --example avisos_terceros' para crearlo.");
                std::process::exit(1);
            }
        }
    } else {
        fs::write(&target_path, &content).expect("fallo al escribir archivo de avisos");
        println!(
            "Archivo generado correctamente en {}: {} crates procesadas ({} bytes).",
            target_path.display(),
            total_crates,
            content.len()
        );
    }
}
